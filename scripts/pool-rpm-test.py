#!/usr/bin/env python3
"""Measure a kiro.rs account pool's sustainable RPM.

This is an open-loop load test: the script schedules requests at a target RPM
instead of waiting for each request to finish before sending the next one. That
is closer to real external traffic and exposes queueing, cooldown, and upstream
429 behavior.

Only Python standard library modules are used.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import queue
import random
import socket
import ssl
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from concurrent.futures import Future, ThreadPoolExecutor, wait
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


DEFAULT_BASE_URL = "http://127.0.0.1:8990"
USER_AGENT = "kiro-rs-pool-rpm-test/1.0"
NAMESPACE = uuid.UUID("46b27160-0266-4f5e-9a62-29656b375a4f")


@dataclass
class RequestResult:
    run_id: str
    stage_index: int
    target_rpm: float
    request_index: int
    session_index: int
    session_id: str
    turn: int
    profile: str
    scheduled_at: float
    worker_started_at: float
    completed_at: float
    client_queue_ms: int
    status: int
    ok: bool
    error_type: str
    error_message: str
    header_ms: int | None
    first_chunk_ms: int | None
    first_event_ms: int | None
    total_ms: int
    event_count: int
    response_bytes: int
    visible_chars: int
    input_tokens: int | None
    output_tokens: int | None
    cache_read_input_tokens: int | None
    cache_creation_input_tokens: int | None
    response_model: str | None


@dataclass
class RuntimeSample:
    run_id: str
    sampled_at: float
    status: int
    error: str
    global_in_flight: int | None
    global_max_concurrent: int | None
    queue_depth: int | None
    queue_max_size: int | None
    total_credentials: int | None
    available_credentials: int | None
    dispatch_available_credentials: int | None
    cooling_down_credentials: int | None
    account_in_flight_sum: int | None
    account_in_flight_max: int | None
    account_max_concurrent_sum: int | None
    effective_rpm_sum: int | None


@dataclass
class StageSummary:
    stage_index: int
    target_rpm: float
    duration_seconds: float
    scheduled: int
    completed: int
    ok: int
    http_429: int
    http_5xx: int
    timeout: int
    other_error: int
    achieved_rpm: float
    success_rate: float
    rate_429: float
    timeout_rate: float
    p50_total_ms: int | None
    p95_total_ms: int | None
    p99_total_ms: int | None
    p50_first_event_ms: int | None
    p95_first_event_ms: int | None
    p95_client_queue_ms: int | None
    max_queue_depth: int | None
    max_global_in_flight: int | None
    max_cooling_down_credentials: int | None
    min_dispatch_available_credentials: int | None
    stable: bool
    stable_reason: str


class PoolRpmTester:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.run_id = args.run_id or time.strftime("rpm-%Y%m%d-%H%M%S")
        self.endpoint = normalize_endpoint(args.base_url, args.route)
        self.admin_url = args.base_url.rstrip("/") + "/api/admin/runtime"
        self.api_key = resolve_api_key(args.api_key, ["API_KEY", "KIRO_RS_API_KEY"])
        self.admin_api_key = resolve_optional_api_key(
            args.admin_api_key,
            ["ADMIN_API_KEY", "KIRO_RS_ADMIN_API_KEY"],
            fallback=self.api_key if args.admin_key_fallback else None,
        )
        self.out_dir = Path(args.out_dir or f"tmp/pool-rpm-{self.run_id}")
        self.results: list[RequestResult] = []
        self.runtime_samples: list[RuntimeSample] = []
        self.results_lock = threading.Lock()
        self.samples_lock = threading.Lock()
        self.session_turns = [0 for _ in range(args.sessions)]
        self.session_lock = threading.Lock()
        self.stop_sampler = threading.Event()
        self.print_lock = threading.Lock()
        self.request_counter = 0
        self.request_counter_lock = threading.Lock()
        self.rng = random.Random(args.seed)

    def run(self) -> int:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        write_json(self.out_dir / "config.json", self.safe_config())

        if self.args.dry_run:
            print(json.dumps(self.safe_config(), ensure_ascii=False, indent=2))
            return 0

        sampler_thread: threading.Thread | None = None
        if self.admin_api_key and self.args.admin_sample_interval > 0:
            sampler_thread = threading.Thread(target=self.sample_runtime_loop, daemon=True)
            sampler_thread.start()

        summaries: list[StageSummary] = []
        with ThreadPoolExecutor(max_workers=self.args.max_workers) as executor:
            if self.args.warmup_seconds > 0 and self.args.warmup_rpm > 0:
                summary = self.run_stage(
                    executor=executor,
                    stage_index=0,
                    target_rpm=self.args.warmup_rpm,
                    duration_seconds=self.args.warmup_seconds,
                    label="warmup",
                )
                summaries.append(summary)

            stage_index = 1
            target = self.args.start_rpm
            while target <= self.args.max_rpm + 1e-9:
                summary = self.run_stage(
                    executor=executor,
                    stage_index=stage_index,
                    target_rpm=target,
                    duration_seconds=self.args.step_seconds,
                    label="test",
                )
                summaries.append(summary)
                if self.args.stop_on_unstable and stage_index > 1 and not summary.stable:
                    self.log(
                        f"stop: stage {stage_index} target_rpm={target:g} is unstable: "
                        f"{summary.stable_reason}"
                    )
                    break
                stage_index += 1
                target += self.args.step_rpm

        self.stop_sampler.set()
        if sampler_thread:
            sampler_thread.join(timeout=5)

        self.write_outputs(summaries)
        self.print_final(summaries)
        return 0

    def run_stage(
        self,
        executor: ThreadPoolExecutor,
        stage_index: int,
        target_rpm: float,
        duration_seconds: float,
        label: str,
    ) -> StageSummary:
        interval = 60.0 / target_rpm
        scheduled_count = max(1, int(math.floor(duration_seconds / interval)))
        stage_started_wall = time.time()
        stage_started = time.monotonic()
        self.log(
            f"stage={stage_index} {label} target_rpm={target_rpm:g} "
            f"duration={duration_seconds:g}s scheduled={scheduled_count}"
        )

        futures: list[Future[None]] = []
        for offset in range(scheduled_count):
            scheduled_mono = stage_started + (offset * interval)
            sleep_for = scheduled_mono - time.monotonic()
            if sleep_for > 0:
                time.sleep(sleep_for)

            request_index = self.next_request_index()
            session_index, session_id, turn = self.next_session(request_index)
            profile = self.pick_profile(request_index)
            scheduled_at = time.time()
            futures.append(
                executor.submit(
                    self.send_one,
                    stage_index,
                    target_rpm,
                    request_index,
                    session_index,
                    session_id,
                    turn,
                    profile,
                    scheduled_at,
                )
            )

        wait_timeout = self.args.timeout_secs + self.args.drain_timeout_secs
        done, pending = wait(futures, timeout=wait_timeout)
        if pending:
            self.log(
                f"warning: stage={stage_index} has {len(pending)} unfinished client tasks "
                f"after {wait_timeout:g}s"
            )

        stage_completed_wall = time.time()
        summary = self.summarize_stage(
            stage_index=stage_index,
            target_rpm=target_rpm,
            scheduled=scheduled_count,
            stage_started_wall=stage_started_wall,
            stage_completed_wall=stage_completed_wall,
        )
        self.print_stage_summary(summary)
        return summary

    def send_one(
        self,
        stage_index: int,
        target_rpm: float,
        request_index: int,
        session_index: int,
        session_id: str,
        turn: int,
        profile: str,
        scheduled_at: float,
    ) -> None:
        worker_started_at = time.time()
        client_queue_ms = ms_since(scheduled_at, worker_started_at)
        payload = build_payload(
            args=self.args,
            run_id=self.run_id,
            request_index=request_index,
            session_index=session_index,
            session_id=session_id,
            turn=turn,
            profile=profile,
        )
        started = time.time()
        status = 0
        ok = False
        error_type = ""
        error_message = ""
        header_ms: int | None = None
        first_chunk_ms: int | None = None
        first_event_ms: int | None = None
        event_count = 0
        response_bytes = 0
        visible_chars = 0
        usage: dict[str, int | None] = {}
        response_model: str | None = None

        try:
            request = urllib.request.Request(
                self.endpoint,
                data=json.dumps(payload, separators=(",", ":")).encode("utf-8"),
                headers=request_headers(self.api_key, self.args.auth),
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=self.args.timeout_secs) as response:
                status = int(response.status)
                header_ms = ms_since(started, time.time())
                if self.args.stream:
                    parsed = parse_sse_response(response, started)
                    first_chunk_ms = parsed["first_chunk_ms"]
                    first_event_ms = parsed["first_event_ms"]
                    event_count = parsed["event_count"]
                    response_bytes = parsed["response_bytes"]
                    visible_chars = parsed["visible_chars"]
                    usage = parsed["usage"]
                    response_model = parsed["response_model"]
                else:
                    body = response.read()
                    response_bytes = len(body)
                    payload_json = json.loads(body.decode("utf-8"))
                    usage = extract_usage(payload_json)
                    response_model = payload_json.get("model")
                    visible_chars = len(extract_text(payload_json))
                ok = 200 <= status < 300
        except urllib.error.HTTPError as exc:
            status = int(exc.code)
            body = exc.read()
            response_bytes = len(body)
            error_type, error_message = parse_error_body(body)
        except (TimeoutError, socket.timeout) as exc:
            status = 0
            error_type = "timeout"
            error_message = str(exc)
        except (urllib.error.URLError, ssl.SSLError, OSError, json.JSONDecodeError) as exc:
            status = 0
            error_type = exc.__class__.__name__
            error_message = str(exc)

        completed_at = time.time()
        if not ok and not error_type:
            error_type = f"http_{status}"

        result = RequestResult(
            run_id=self.run_id,
            stage_index=stage_index,
            target_rpm=target_rpm,
            request_index=request_index,
            session_index=session_index,
            session_id=session_id,
            turn=turn,
            profile=profile,
            scheduled_at=scheduled_at,
            worker_started_at=worker_started_at,
            completed_at=completed_at,
            client_queue_ms=client_queue_ms,
            status=status,
            ok=ok,
            error_type=error_type,
            error_message=error_message[:500],
            header_ms=header_ms,
            first_chunk_ms=first_chunk_ms,
            first_event_ms=first_event_ms,
            total_ms=ms_since(started, completed_at),
            event_count=event_count,
            response_bytes=response_bytes,
            visible_chars=visible_chars,
            input_tokens=usage.get("input_tokens"),
            output_tokens=usage.get("output_tokens"),
            cache_read_input_tokens=usage.get("cache_read_input_tokens"),
            cache_creation_input_tokens=usage.get("cache_creation_input_tokens"),
            response_model=response_model if isinstance(response_model, str) else None,
        )
        with self.results_lock:
            self.results.append(result)

    def sample_runtime_loop(self) -> None:
        while not self.stop_sampler.is_set():
            sample = self.fetch_runtime_sample()
            with self.samples_lock:
                self.runtime_samples.append(sample)
            self.stop_sampler.wait(self.args.admin_sample_interval)

    def fetch_runtime_sample(self) -> RuntimeSample:
        sampled_at = time.time()
        status = 0
        error = ""
        body: dict[str, Any] = {}
        try:
            request = urllib.request.Request(
                self.admin_url,
                headers=request_headers(self.admin_api_key or "", "x-api-key"),
                method="GET",
            )
            with urllib.request.urlopen(request, timeout=self.args.admin_timeout_secs) as response:
                status = int(response.status)
                body = json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            status = int(exc.code)
            error = parse_error_body(exc.read())[1]
        except (TimeoutError, socket.timeout, urllib.error.URLError, OSError, json.JSONDecodeError) as exc:
            error = str(exc)

        credentials = body.get("credentials") if isinstance(body.get("credentials"), list) else []
        in_flights = [int_or_none(item.get("inFlight")) for item in credentials if isinstance(item, dict)]
        max_concurrents = [
            int_or_none(item.get("maxConcurrent")) for item in credentials if isinstance(item, dict)
        ]
        effective_rpms = [
            int_or_none(item.get("effectiveRpm")) for item in credentials if isinstance(item, dict)
        ]
        in_flights_clean = [value for value in in_flights if value is not None]
        max_concurrents_clean = [value for value in max_concurrents if value is not None]
        effective_rpms_clean = [value for value in effective_rpms if value is not None and value > 0]

        return RuntimeSample(
            run_id=self.run_id,
            sampled_at=sampled_at,
            status=status,
            error=error[:500],
            global_in_flight=int_or_none(body.get("globalInFlight")),
            global_max_concurrent=int_or_none(body.get("globalMaxConcurrent")),
            queue_depth=int_or_none(body.get("queueDepth")),
            queue_max_size=int_or_none(body.get("queueMaxSize")),
            total_credentials=int_or_none(body.get("totalCredentials")),
            available_credentials=int_or_none(body.get("availableCredentials")),
            dispatch_available_credentials=int_or_none(body.get("dispatchAvailableCredentials")),
            cooling_down_credentials=int_or_none(body.get("coolingDownCredentials")),
            account_in_flight_sum=sum(in_flights_clean) if in_flights_clean else None,
            account_in_flight_max=max(in_flights_clean) if in_flights_clean else None,
            account_max_concurrent_sum=sum(max_concurrents_clean) if max_concurrents_clean else None,
            effective_rpm_sum=sum(effective_rpms_clean) if effective_rpms_clean else None,
        )

    def summarize_stage(
        self,
        stage_index: int,
        target_rpm: float,
        scheduled: int,
        stage_started_wall: float,
        stage_completed_wall: float,
    ) -> StageSummary:
        with self.results_lock:
            results = [item for item in self.results if item.stage_index == stage_index]
        with self.samples_lock:
            samples = [
                item
                for item in self.runtime_samples
                if stage_started_wall <= item.sampled_at <= stage_completed_wall
            ]

        completed = len(results)
        ok = sum(1 for item in results if item.ok)
        http_429 = sum(1 for item in results if item.status == 429)
        http_5xx = sum(1 for item in results if item.status >= 500)
        timeout = sum(1 for item in results if item.error_type == "timeout")
        other_error = completed - ok - http_429 - http_5xx - timeout
        success_rate = safe_div(ok, completed)
        rate_429 = safe_div(http_429, completed)
        timeout_rate = safe_div(timeout, completed)
        achieved_rpm = ok * 60.0 / max(self.args.step_seconds, 1.0)
        if stage_index == 0 and self.args.warmup_seconds > 0:
            achieved_rpm = ok * 60.0 / max(self.args.warmup_seconds, 1.0)

        total_latencies = [item.total_ms for item in results if item.total_ms is not None]
        first_events = [item.first_event_ms for item in results if item.first_event_ms is not None]
        client_queues = [item.client_queue_ms for item in results]
        max_queue_depth = max_optional(item.queue_depth for item in samples)
        max_global_in_flight = max_optional(item.global_in_flight for item in samples)
        max_cooling = max_optional(item.cooling_down_credentials for item in samples)
        min_dispatch = min_optional(item.dispatch_available_credentials for item in samples)

        stable, reason = self.is_stable(
            completed=completed,
            scheduled=scheduled,
            success_rate=success_rate,
            rate_429=rate_429,
            timeout_rate=timeout_rate,
            http_5xx=http_5xx,
            achieved_rpm=achieved_rpm,
            target_rpm=target_rpm,
            p95_total_ms=percentile(total_latencies, 0.95),
        )

        return StageSummary(
            stage_index=stage_index,
            target_rpm=target_rpm,
            duration_seconds=stage_completed_wall - stage_started_wall,
            scheduled=scheduled,
            completed=completed,
            ok=ok,
            http_429=http_429,
            http_5xx=http_5xx,
            timeout=timeout,
            other_error=other_error,
            achieved_rpm=round(achieved_rpm, 2),
            success_rate=round(success_rate, 4),
            rate_429=round(rate_429, 4),
            timeout_rate=round(timeout_rate, 4),
            p50_total_ms=percentile(total_latencies, 0.50),
            p95_total_ms=percentile(total_latencies, 0.95),
            p99_total_ms=percentile(total_latencies, 0.99),
            p50_first_event_ms=percentile(first_events, 0.50),
            p95_first_event_ms=percentile(first_events, 0.95),
            p95_client_queue_ms=percentile(client_queues, 0.95),
            max_queue_depth=max_queue_depth,
            max_global_in_flight=max_global_in_flight,
            max_cooling_down_credentials=max_cooling,
            min_dispatch_available_credentials=min_dispatch,
            stable=stable,
            stable_reason=reason,
        )

    def is_stable(
        self,
        completed: int,
        scheduled: int,
        success_rate: float,
        rate_429: float,
        timeout_rate: float,
        http_5xx: int,
        achieved_rpm: float,
        target_rpm: float,
        p95_total_ms: int | None,
    ) -> tuple[bool, str]:
        if completed < scheduled:
            return False, "client did not complete all scheduled requests"
        if success_rate < self.args.min_success_rate:
            return False, f"success_rate {success_rate:.4f} < {self.args.min_success_rate:.4f}"
        if rate_429 > self.args.max_429_rate:
            return False, f"429_rate {rate_429:.4f} > {self.args.max_429_rate:.4f}"
        if timeout_rate > self.args.max_timeout_rate:
            return False, f"timeout_rate {timeout_rate:.4f} > {self.args.max_timeout_rate:.4f}"
        if safe_div(http_5xx, completed) > self.args.max_5xx_rate:
            return False, f"5xx_rate {safe_div(http_5xx, completed):.4f} > {self.args.max_5xx_rate:.4f}"
        if achieved_rpm < target_rpm * self.args.min_achieved_ratio:
            return False, f"achieved_rpm {achieved_rpm:.2f} below target"
        if self.args.p95_sla_ms > 0 and p95_total_ms is not None and p95_total_ms > self.args.p95_sla_ms:
            return False, f"p95_total_ms {p95_total_ms} > {self.args.p95_sla_ms}"
        return True, "ok"

    def write_outputs(self, summaries: list[StageSummary]) -> None:
        with self.results_lock:
            results = list(self.results)
        with self.samples_lock:
            samples = list(self.runtime_samples)

        write_csv(self.out_dir / "requests.csv", [asdict(item) for item in results])
        write_csv(self.out_dir / "runtime.csv", [asdict(item) for item in samples])
        write_csv(self.out_dir / "summary.csv", [asdict(item) for item in summaries])

        failures = [asdict(item) for item in results if not item.ok]
        with (self.out_dir / "failures.jsonl").open("w", encoding="utf-8") as fh:
            for item in failures:
                fh.write(json.dumps(item, ensure_ascii=False, separators=(",", ":")) + "\n")

        highest_stable = highest_stable_rpm(summaries)
        report = {
            "run_id": self.run_id,
            "endpoint": self.endpoint,
            "model": self.args.model,
            "profile": self.args.profile,
            "stream": self.args.stream,
            "highest_stable_rpm": highest_stable,
            "recommended_safe_rpm": round(highest_stable * self.args.safety_factor, 2)
            if highest_stable
            else 0,
            "safety_factor": self.args.safety_factor,
            "summaries": [asdict(item) for item in summaries],
            "output_files": {
                "requests": str(self.out_dir / "requests.csv"),
                "runtime": str(self.out_dir / "runtime.csv"),
                "summary": str(self.out_dir / "summary.csv"),
                "failures": str(self.out_dir / "failures.jsonl"),
            },
        }
        write_json(self.out_dir / "summary.json", report)

    def print_stage_summary(self, summary: StageSummary) -> None:
        self.log(
            "stage={stage} target={target:g}rpm ok={ok}/{completed} "
            "achieved={achieved:.2f}rpm success={success:.2%} 429={rate429:.2%} "
            "p95={p95}ms first_event_p95={first}ms queue_max={queue} stable={stable} {reason}".format(
                stage=summary.stage_index,
                target=summary.target_rpm,
                ok=summary.ok,
                completed=summary.completed,
                achieved=summary.achieved_rpm,
                success=summary.success_rate,
                rate429=summary.rate_429,
                p95=summary.p95_total_ms,
                first=summary.p95_first_event_ms,
                queue=summary.max_queue_depth,
                stable=summary.stable,
                reason=summary.stable_reason,
            )
        )

    def print_final(self, summaries: list[StageSummary]) -> None:
        highest = highest_stable_rpm(summaries)
        safe = round(highest * self.args.safety_factor, 2) if highest else 0
        print()
        print("Final")
        print(f"output_dir={self.out_dir}")
        print(f"highest_stable_rpm={highest:g}")
        print(f"recommended_safe_rpm={safe:g} safety_factor={self.args.safety_factor:g}")
        print(f"summary_json={self.out_dir / 'summary.json'}")
        print(f"requests_csv={self.out_dir / 'requests.csv'}")
        if not self.admin_api_key:
            print("admin_sampling=disabled (set ADMIN_API_KEY or --admin-api-key to enable)")

    def next_request_index(self) -> int:
        with self.request_counter_lock:
            self.request_counter += 1
            return self.request_counter

    def next_session(self, request_index: int) -> tuple[int, str, int]:
        session_index = (request_index - 1) % self.args.sessions
        with self.session_lock:
            self.session_turns[session_index] += 1
            turn = self.session_turns[session_index]
        session_id = str(uuid.uuid5(NAMESPACE, f"{self.run_id}:session:{session_index}"))
        return session_index, session_id, turn

    def pick_profile(self, request_index: int) -> str:
        if self.args.profile != "mixed-agent":
            return self.args.profile
        rng = random.Random(self.args.seed + request_index)
        value = rng.random()
        if value < 0.50:
            return "short-chat"
        if value < 0.90:
            return "coding-agent"
        return "heavy-context"

    def log(self, message: str) -> None:
        with self.print_lock:
            print(time.strftime("%H:%M:%S"), message, flush=True)

    def safe_config(self) -> dict[str, Any]:
        data = vars(self.args).copy()
        data["api_key"] = mask_secret(self.api_key)
        data["admin_api_key"] = mask_secret(self.admin_api_key)
        data["endpoint"] = self.endpoint
        data["admin_url"] = self.admin_url
        data["run_id"] = self.run_id
        return data


def build_payload(
    args: argparse.Namespace,
    run_id: str,
    request_index: int,
    session_index: int,
    session_id: str,
    turn: int,
    profile: str,
) -> dict[str, Any]:
    max_tokens = args.max_tokens
    system_text = (
        "You are assisting with a production load test. Answer naturally and "
        "briefly. Do not mention that this is synthetic unless asked."
    )
    messages = build_messages(profile, request_index, session_index, turn, args.history_turns, args)
    payload: dict[str, Any] = {
        "model": args.model,
        "max_tokens": max_tokens,
        "stream": args.stream,
        "system": [
            {
                "text": system_text,
                "cache_control": {"type": "ephemeral", "ttl": "5m"},
            }
        ],
        "metadata": {
            "user_id": json.dumps(
                {
                    "device_id": f"rpm-device-{session_index % args.device_count}",
                    "account_uuid": args.account_uuid,
                    "user_id": f"pool-user-{session_index % args.user_count}",
                    "session_id": session_id,
                },
                separators=(",", ":"),
            )
        },
        "messages": messages,
    }

    if profile in {"coding-agent", "heavy-context"} or args.include_tools:
        payload["tools"] = synthetic_tools()

    if args.thinking != "off":
        payload["thinking"] = {
            "type": args.thinking,
            "budget_tokens": args.thinking_budget_tokens,
        }
        if args.effort:
            payload["output_config"] = {"effort": args.effort}

    return payload


def build_messages(
    profile: str,
    request_index: int,
    session_index: int,
    turn: int,
    history_turns: int,
    args: argparse.Namespace,
) -> list[dict[str, Any]]:
    messages: list[dict[str, Any]] = []
    previous = max(0, min(history_turns, turn - 1))
    for idx in range(previous):
        messages.append(
            {
                "role": "user",
                "content": (
                    f"Previous task {idx + 1} for session {session_index}: "
                    "review a small code change and identify the main risk."
                ),
            }
        )
        messages.append(
            {
                "role": "assistant",
                "content": (
                    "The main risk is behavior drift around error handling. "
                    "Add a focused regression test and keep the patch small."
                ),
            }
        )

    if profile == "short-chat":
        prompt = (
            f"Request {request_index}. Summarize the tradeoff between lower latency "
            "and higher model quality in three concise bullet points."
        )
    elif profile == "coding-agent":
        code = synthetic_code_block(lines=90)
        prompt = (
            "Review this Rust-like handler and propose a minimal fix plan. "
            "Do not call tools; answer in compact engineering prose.\n\n"
            f"```rust\n{code}\n```"
        )
    elif profile == "heavy-context":
        context = synthetic_heavy_context(args.heavy_context_kb)
        prompt = (
            "You are continuing a long coding session. Use the context below to "
            "identify the top three operational risks and the next test to add. "
            "Keep the answer under 400 words.\n\n"
            f"{context}"
        )
    else:
        raise ValueError(f"unknown profile: {profile}")

    messages.append({"role": "user", "content": prompt})
    return messages


def synthetic_code_block(lines: int) -> str:
    chunks = []
    for idx in range(lines):
        chunks.append(
            f"fn handle_case_{idx}(input: &Request) -> Result<Response> {{ "
            f"validate(input)?; dispatch(input, {idx}) }}"
        )
    return "\n".join(chunks)


def synthetic_heavy_context(kb: int) -> str:
    unit = (
        "Module notes: request routing, streaming usage accounting, account pool "
        "selection, cooldown handling, and runtime observability must remain "
        "consistent under load. The test should preserve session identity and "
        "measure queueing, first-event latency, total latency, and error rates.\n"
    )
    target = max(1, kb) * 1024
    repeat = max(1, math.ceil(target / len(unit)))
    return (unit * repeat)[:target]


def synthetic_tools() -> list[dict[str, Any]]:
    return [
        {
            "name": "read_file",
            "description": "Read a UTF-8 text file from the workspace.",
            "input_schema": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            },
            "cache_control": {"type": "ephemeral", "ttl": "5m"},
        },
        {
            "name": "run_tests",
            "description": "Run a named test command and return its exit status.",
            "input_schema": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"],
            },
        },
        {
            "name": "apply_patch",
            "description": "Apply a small source patch.",
            "input_schema": {
                "type": "object",
                "properties": {"patch": {"type": "string"}},
                "required": ["patch"],
            },
        },
    ]


def parse_sse_response(response: Any, started: float) -> dict[str, Any]:
    first_chunk_ms: int | None = None
    first_event_ms: int | None = None
    event_name = ""
    data_lines: list[str] = []
    event_count = 0
    response_bytes = 0
    visible_chars = 0
    usage: dict[str, int | None] = {}
    response_model: str | None = None

    for raw in response:
        now = time.time()
        if first_chunk_ms is None:
            first_chunk_ms = ms_since(started, now)
        response_bytes += len(raw)
        line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
        if line == "":
            if data_lines:
                event_count += 1
                if first_event_ms is None:
                    first_event_ms = ms_since(started, now)
                data = "\n".join(data_lines)
                if data.strip() != "[DONE]":
                    try:
                        obj = json.loads(data)
                    except json.JSONDecodeError:
                        obj = {}
                    merge_usage(usage, obj)
                    if isinstance(obj.get("model"), str):
                        response_model = obj["model"]
                    if isinstance(obj.get("message"), dict) and isinstance(obj["message"].get("model"), str):
                        response_model = obj["message"]["model"]
                    visible_chars += len(extract_text(obj))
            event_name = ""
            data_lines = []
            continue
        if line.startswith("event:"):
            event_name = line[6:].strip()
        elif line.startswith("data:"):
            data_lines.append(line[5:].lstrip())

    if data_lines:
        event_count += 1
    return {
        "first_chunk_ms": first_chunk_ms,
        "first_event_ms": first_event_ms,
        "event_count": event_count,
        "response_bytes": response_bytes,
        "visible_chars": visible_chars,
        "usage": usage,
        "response_model": response_model or event_name or None,
    }


def merge_usage(target: dict[str, int | None], obj: Any) -> None:
    for usage in find_usage_dicts(obj):
        for key in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"):
            value = int_or_none(usage.get(key))
            if value is not None:
                target[key] = value
        cache_creation = usage.get("cache_creation")
        if isinstance(cache_creation, dict):
            total = 0
            found = False
            for value in cache_creation.values():
                parsed = int_or_none(value)
                if parsed is not None:
                    total += parsed
                    found = True
            if found:
                target["cache_creation_input_tokens"] = total


def find_usage_dicts(obj: Any) -> list[dict[str, Any]]:
    found: list[dict[str, Any]] = []
    if isinstance(obj, dict):
        usage = obj.get("usage")
        if isinstance(usage, dict):
            found.append(usage)
        message = obj.get("message")
        if isinstance(message, dict) and isinstance(message.get("usage"), dict):
            found.append(message["usage"])
        delta = obj.get("delta")
        if isinstance(delta, dict) and isinstance(delta.get("usage"), dict):
            found.append(delta["usage"])
    return found


def extract_usage(obj: dict[str, Any]) -> dict[str, int | None]:
    usage: dict[str, int | None] = {}
    merge_usage(usage, obj)
    return usage


def extract_text(obj: Any) -> str:
    parts: list[str] = []
    if isinstance(obj, dict):
        delta = obj.get("delta")
        if isinstance(delta, dict):
            text = delta.get("text")
            if isinstance(text, str):
                parts.append(text)
            thinking = delta.get("thinking")
            if isinstance(thinking, str):
                parts.append(thinking)
        content = obj.get("content")
        if isinstance(content, list):
            for item in content:
                if isinstance(item, dict) and isinstance(item.get("text"), str):
                    parts.append(item["text"])
        if isinstance(obj.get("text"), str):
            parts.append(obj["text"])
    return "".join(parts)


def normalize_endpoint(base_url: str, route: str) -> str:
    base = base_url.rstrip("/")
    if route.startswith("http://") or route.startswith("https://"):
        return route
    return base + "/" + route.lstrip("/")


def request_headers(api_key: str, auth: str) -> dict[str, str]:
    headers = {
        "Content-Type": "application/json",
        "User-Agent": USER_AGENT,
        "anthropic-version": "2023-06-01",
    }
    if auth in {"x-api-key", "both"}:
        headers["x-api-key"] = api_key
    if auth in {"bearer", "both"}:
        headers["Authorization"] = f"Bearer {api_key}"
    return headers


def parse_error_body(body: bytes) -> tuple[str, str]:
    if not body:
        return "", ""
    text = body.decode("utf-8", errors="replace")
    try:
        obj = json.loads(text)
    except json.JSONDecodeError:
        return "http_error", text[:500]
    error = obj.get("error")
    if isinstance(error, dict):
        return str(error.get("type") or "http_error"), str(error.get("message") or "")[:500]
    return "http_error", text[:500]


def resolve_api_key(value: str | None, env_names: list[str]) -> str:
    resolved = resolve_optional_api_key(value, env_names, fallback=None)
    if not resolved:
        names = ", ".join(env_names)
        raise SystemExit(f"missing API key: pass --api-key or set one of {names}")
    return resolved


def resolve_optional_api_key(value: str | None, env_names: list[str], fallback: str | None) -> str | None:
    if value:
        if value.startswith("env:"):
            name = value[4:]
            secret = os.environ.get(name)
            if not secret:
                raise SystemExit(f"missing environment variable: {name}")
            return secret
        return value
    for name in env_names:
        secret = os.environ.get(name)
        if secret:
            return secret
    return fallback


def mask_secret(value: str | None) -> str | None:
    if not value:
        return None
    if len(value) <= 8:
        return "***"
    return value[:4] + "***" + value[-4:]


def ms_since(start: float, end: float) -> int:
    return int(round((end - start) * 1000))


def int_or_none(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, float) and value.is_integer():
        return int(value)
    if isinstance(value, str) and value.strip().isdigit():
        return int(value.strip())
    return None


def safe_div(numerator: float, denominator: float) -> float:
    if denominator <= 0:
        return 0.0
    return numerator / denominator


def percentile(values: list[int], ratio: float) -> int | None:
    if not values:
        return None
    ordered = sorted(values)
    index = math.ceil(len(ordered) * ratio) - 1
    index = min(max(index, 0), len(ordered) - 1)
    return ordered[index]


def max_optional(values: Any) -> int | None:
    clean = [value for value in values if value is not None]
    return max(clean) if clean else None


def min_optional(values: Any) -> int | None:
    clean = [value for value in values if value is not None]
    return min(clean) if clean else None


def highest_stable_rpm(summaries: list[StageSummary]) -> float:
    stable = [item.target_rpm for item in summaries if item.stage_index > 0 and item.stable]
    return max(stable) if stable else 0.0


def write_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    if not rows:
        path.write_text("", encoding="utf-8")
        return
    with path.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
        writer.writeheader()
        writer.writerows(rows)


def write_json(path: Path, data: Any) -> None:
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default=os.environ.get("BASE_URL", DEFAULT_BASE_URL))
    parser.add_argument("--route", default="/v1/messages")
    parser.add_argument("--api-key", default=None, help="API key literal or env:NAME. Defaults to API_KEY/KIRO_RS_API_KEY.")
    parser.add_argument(
        "--admin-api-key",
        default=None,
        help="Admin API key literal or env:NAME. Defaults to ADMIN_API_KEY/KIRO_RS_ADMIN_API_KEY.",
    )
    parser.add_argument("--admin-key-fallback", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--auth", choices=["x-api-key", "bearer", "both"], default="x-api-key")
    parser.add_argument("--model", default="claude-opus-4-7")
    parser.add_argument("--stream", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--profile", choices=["short-chat", "coding-agent", "heavy-context", "mixed-agent"], default="mixed-agent")
    parser.add_argument("--start-rpm", type=float, default=10.0)
    parser.add_argument("--step-rpm", type=float, default=10.0)
    parser.add_argument("--max-rpm", type=float, default=100.0)
    parser.add_argument("--step-seconds", type=float, default=120.0)
    parser.add_argument("--warmup-rpm", type=float, default=5.0)
    parser.add_argument("--warmup-seconds", type=float, default=30.0)
    parser.add_argument("--sessions", type=int, default=100)
    parser.add_argument("--device-count", type=int, default=50)
    parser.add_argument("--user-count", type=int, default=100)
    parser.add_argument("--account-uuid", default="pool-rpm-test")
    parser.add_argument("--history-turns", type=int, default=3)
    parser.add_argument("--heavy-context-kb", type=int, default=64)
    parser.add_argument("--max-tokens", type=int, default=2048)
    parser.add_argument("--include-tools", action="store_true")
    parser.add_argument("--thinking", choices=["off", "enabled", "adaptive"], default="off")
    parser.add_argument("--thinking-budget-tokens", type=int, default=20000)
    parser.add_argument("--effort", default="high")
    parser.add_argument("--timeout-secs", type=float, default=180.0)
    parser.add_argument("--drain-timeout-secs", type=float, default=30.0)
    parser.add_argument("--max-workers", type=int, default=256)
    parser.add_argument("--admin-sample-interval", type=float, default=2.0)
    parser.add_argument("--admin-timeout-secs", type=float, default=5.0)
    parser.add_argument("--min-success-rate", type=float, default=0.99)
    parser.add_argument("--max-429-rate", type=float, default=0.01)
    parser.add_argument("--max-5xx-rate", type=float, default=0.005)
    parser.add_argument("--max-timeout-rate", type=float, default=0.0)
    parser.add_argument("--min-achieved-ratio", type=float, default=0.95)
    parser.add_argument("--p95-sla-ms", type=int, default=0)
    parser.add_argument("--safety-factor", type=float, default=0.85)
    parser.add_argument("--stop-on-unstable", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--seed", type=int, default=20260521)
    parser.add_argument("--run-id", default="")
    parser.add_argument("--out-dir", default="")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    validate_args(args)
    return args


def validate_args(args: argparse.Namespace) -> None:
    if args.start_rpm <= 0 or args.step_rpm <= 0 or args.max_rpm <= 0:
        raise SystemExit("RPM values must be positive")
    if args.step_seconds <= 0:
        raise SystemExit("--step-seconds must be positive")
    if args.sessions <= 0:
        raise SystemExit("--sessions must be positive")
    if args.max_workers <= 0:
        raise SystemExit("--max-workers must be positive")
    if args.max_rpm < args.start_rpm:
        raise SystemExit("--max-rpm must be >= --start-rpm")


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    return PoolRpmTester(args).run()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
