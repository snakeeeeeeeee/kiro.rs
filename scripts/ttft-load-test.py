#!/usr/bin/env python3
"""Concurrent TTFT load test for the Anthropic-compatible /v1/messages API.

The script uses streaming by default because true time-to-first-token needs an
SSE response. Non-streaming mode is supported for total latency only.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import socket
import ssl
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from concurrent.futures import Future, ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


DEFAULT_BASE_URL = "http://127.0.0.1:8990"
DEFAULT_API_KEY = "z123456789"
DEFAULT_MODEL = "claude-opus-4-8"
DEFAULT_ROUTE = "/v1/messages"
USER_AGENT = "kiro-rs-ttft-load-test/1.0"


@dataclass
class RequestResult:
    request_index: int
    stage_index: int
    stage_concurrency: int
    stage_target_rpm: float
    ok: bool
    status: int
    error_type: str
    error_message: str
    scheduled_at: float
    started_at: float
    completed_at: float
    client_queue_ms: int
    header_ms: int | None
    first_chunk_ms: int | None
    first_event_ms: int | None
    first_token_ms: int | None
    total_ms: int
    event_count: int
    response_bytes: int
    visible_chars: int
    input_tokens: int | None
    output_tokens: int | None
    response_model: str | None


@dataclass
class StageSummary:
    stage_index: int
    concurrency: int
    target_rpm: float
    started_at: float
    completed_at: float
    wall_ms: int
    measure_seconds: float
    scheduled: int
    completed: int
    ok: int
    errors: int
    success_rate: float
    error_rate: float
    completed_rpm: float
    achieved_rpm: float
    p50_ttft_ms: int | None
    p95_ttft_ms: int | None
    p99_ttft_ms: int | None
    p50_total_ms: int | None
    p95_total_ms: int | None
    p99_total_ms: int | None
    estimated_avg_supported_concurrency: float | None
    estimated_p95_supported_concurrency: float | None
    p95_client_queue_ms: int | None
    stable: bool
    stable_reason: str


class TtftLoadTester:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.run_id = args.run_id or time.strftime("ttft-%Y%m%d-%H%M%S")
        self.endpoint = normalize_endpoint(args.base_url, args.route)
        self.api_key = resolve_api_key(args.api_key)
        self.out_dir = Path(args.out_dir or f"tmp/ttft-load-{self.run_id}")
        self.print_lock = threading.Lock()
        self.request_counter = 0
        self.request_counter_lock = threading.Lock()

    def run(self) -> int:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        write_json(self.out_dir / "config.json", self.safe_config())

        if self.args.stream is False:
            self.log("warning: --no-stream cannot measure true first-token latency; TTFT will be empty")

        self.print_header()
        if self.args.warmup_requests > 0:
            self.log(f"warmup requests={self.args.warmup_requests} concurrency={self.args.start_concurrency}")
            self.run_fixed_count(
                request_count=self.args.warmup_requests,
                concurrency=self.args.start_concurrency,
                stage_index=0,
                target_rpm=0.0,
                include_results=False,
            )

        started_wall = time.time()
        if self.args.mode == "fixed":
            results, stage_summaries = self.run_fixed_mode()
        elif self.args.mode == "ramp":
            results, stage_summaries = self.run_ramp_mode()
        else:
            results, stage_summaries = self.run_rpm_ramp_mode()
        completed_wall = time.time()

        summary = build_summary(
            args=self.args,
            run_id=self.run_id,
            endpoint=self.endpoint,
            started_wall=started_wall,
            completed_wall=completed_wall,
            results=results,
            stage_summaries=stage_summaries,
        )
        self.write_outputs(results, stage_summaries, summary)
        print_summary(summary, self.out_dir)

        if self.args.fail_on_error and summary["errors"]["total"] > 0:
            return 1
        return 0

    def run_fixed_mode(self) -> tuple[list[RequestResult], list[StageSummary]]:
        started_at = time.time()
        results = self.run_fixed_count(
            request_count=self.args.requests,
            concurrency=self.args.concurrency,
            stage_index=1,
            target_rpm=0.0,
            include_results=True,
        )
        completed_at = time.time()
        stage = summarize_stage(
            stage_index=1,
            concurrency=self.args.concurrency,
            target_rpm=0.0,
            scheduled=len(results),
            measure_seconds=None,
            started_at=started_at,
            completed_at=completed_at,
            results=results,
            args=self.args,
        )
        self.print_stage_summary(stage)
        return results, [stage]

    def run_ramp_mode(self) -> tuple[list[RequestResult], list[StageSummary]]:
        all_results: list[RequestResult] = []
        stage_summaries: list[StageSummary] = []
        stage_index = 1
        concurrency = self.args.start_concurrency

        while concurrency <= self.args.max_concurrency:
            self.log(
                f"stage={stage_index} concurrency={concurrency} "
                f"duration={self.args.stage_seconds:g}s"
            )
            started_at = time.time()
            stage_results = self.run_timed_stage(
                duration_seconds=self.args.stage_seconds,
                concurrency=concurrency,
                stage_index=stage_index,
                target_rpm=0.0,
            )
            completed_at = time.time()
            all_results.extend(stage_results)
            stage = summarize_stage(
                stage_index=stage_index,
                concurrency=concurrency,
                target_rpm=0.0,
                scheduled=len(stage_results),
                measure_seconds=None,
                started_at=started_at,
                completed_at=completed_at,
                results=stage_results,
                args=self.args,
            )
            stage_summaries.append(stage)
            self.print_stage_summary(stage)

            if self.args.stop_on_unstable and not stage.stable:
                self.log(f"stop: stage={stage_index} unstable: {stage.stable_reason}")
                break

            stage_index += 1
            concurrency += self.args.step_concurrency

        return all_results, stage_summaries

    def run_rpm_ramp_mode(self) -> tuple[list[RequestResult], list[StageSummary]]:
        all_results: list[RequestResult] = []
        stage_summaries: list[StageSummary] = []
        stage_index = 1
        target_rpm = self.args.start_rpm

        while target_rpm <= self.args.max_rpm + 1e-9:
            self.log(
                f"stage={stage_index} target_rpm={target_rpm:g} "
                f"max_workers={self.args.max_workers} duration={self.args.stage_seconds:g}s"
            )
            started_at = time.time()
            stage_results, scheduled = self.run_rpm_stage(
                duration_seconds=self.args.stage_seconds,
                target_rpm=target_rpm,
                stage_index=stage_index,
            )
            completed_at = time.time()
            all_results.extend(stage_results)
            stage = summarize_stage(
                stage_index=stage_index,
                concurrency=max((item.stage_concurrency for item in stage_results), default=0),
                target_rpm=target_rpm,
                scheduled=scheduled,
                measure_seconds=self.args.stage_seconds,
                started_at=started_at,
                completed_at=completed_at,
                results=stage_results,
                args=self.args,
            )
            stage_summaries.append(stage)
            self.print_stage_summary(stage)

            if self.args.stop_on_unstable and not stage.stable:
                self.log(f"stop: stage={stage_index} unstable: {stage.stable_reason}")
                break

            stage_index += 1
            target_rpm += self.args.step_rpm

        return all_results, stage_summaries

    def run_fixed_count(
        self,
        request_count: int,
        concurrency: int,
        stage_index: int,
        target_rpm: float,
        include_results: bool,
    ) -> list[RequestResult]:
        batch_started = time.time()
        futures: list[Future[RequestResult]] = []
        with ThreadPoolExecutor(max_workers=concurrency) as executor:
            for _ in range(request_count):
                idx = self.next_request_index()
                scheduled_at = time.time()
                futures.append(
                    executor.submit(
                        self.send_one,
                        idx,
                        stage_index,
                        concurrency,
                        target_rpm,
                        scheduled_at,
                    )
                )

            results: list[RequestResult] = []
            completed = 0
            for future in as_completed(futures):
                completed += 1
                result = future.result()
                if include_results:
                    results.append(result)
                if self.args.progress_every > 0 and (
                    completed == request_count or completed % self.args.progress_every == 0
                ):
                    elapsed = time.time() - batch_started
                    ok = sum(1 for item in results if item.ok) if include_results else completed
                    self.log(f"progress {completed}/{request_count} ok={ok} elapsed={elapsed:.1f}s")
        return sorted(results, key=lambda item: item.request_index)

    def run_timed_stage(
        self,
        duration_seconds: float,
        concurrency: int,
        stage_index: int,
        target_rpm: float,
    ) -> list[RequestResult]:
        stage_started = time.time()
        stop_at = stage_started + duration_seconds
        results: list[RequestResult] = []

        with ThreadPoolExecutor(max_workers=concurrency) as executor:
            futures: set[Future[RequestResult]] = set()
            while time.time() < stop_at or futures:
                while time.time() < stop_at and len(futures) < concurrency:
                    idx = self.next_request_index()
                    scheduled_at = time.time()
                    futures.add(
                        executor.submit(
                            self.send_one,
                            idx,
                            stage_index,
                            concurrency,
                            target_rpm,
                            scheduled_at,
                        )
                    )

                done = [future for future in futures if future.done()]
                if not done:
                    time.sleep(0.02)
                    continue

                for future in done:
                    futures.remove(future)
                    results.append(future.result())
                    if self.args.progress_every > 0 and len(results) % self.args.progress_every == 0:
                        elapsed = time.time() - stage_started
                        ok = sum(1 for item in results if item.ok)
                        self.log(
                            f"stage={stage_index} progress completed={len(results)} "
                            f"ok={ok} elapsed={elapsed:.1f}s"
                        )

        return sorted(results, key=lambda item: item.request_index)

    def run_rpm_stage(
        self,
        duration_seconds: float,
        target_rpm: float,
        stage_index: int,
    ) -> tuple[list[RequestResult], int]:
        interval = 60.0 / target_rpm
        scheduled_count = max(1, int(math.floor(duration_seconds / interval)))
        stage_started = time.monotonic()
        results: list[RequestResult] = []
        futures: list[Future[RequestResult]] = []

        with ThreadPoolExecutor(max_workers=self.args.max_workers) as executor:
            for offset in range(scheduled_count):
                scheduled_mono = stage_started + (offset * interval)
                sleep_for = scheduled_mono - time.monotonic()
                if sleep_for > 0:
                    time.sleep(sleep_for)

                idx = self.next_request_index()
                scheduled_at = time.time()
                futures.append(
                    executor.submit(
                        self.send_one,
                        idx,
                        stage_index,
                        self.args.max_workers,
                        target_rpm,
                        scheduled_at,
                    )
                )

            remaining = (stage_started + duration_seconds) - time.monotonic()
            if remaining > 0:
                time.sleep(remaining)

            completed = 0
            for future in as_completed(futures):
                completed += 1
                results.append(future.result())
                if self.args.progress_every > 0 and (
                    completed == scheduled_count or completed % self.args.progress_every == 0
                ):
                    elapsed = time.monotonic() - stage_started
                    ok = sum(1 for item in results if item.ok)
                    self.log(
                        f"stage={stage_index} progress completed={completed}/{scheduled_count} "
                        f"ok={ok} elapsed={elapsed:.1f}s"
                    )

        return sorted(results, key=lambda item: item.request_index), scheduled_count

    def send_one(
        self,
        request_index: int,
        stage_index: int,
        stage_concurrency: int,
        stage_target_rpm: float,
        scheduled_at: float,
    ) -> RequestResult:
        started_at = time.time()
        payload = build_payload(self.args, self.run_id, request_index)
        status = 0
        ok = False
        error_type = ""
        error_message = ""
        header_ms: int | None = None
        first_chunk_ms: int | None = None
        first_event_ms: int | None = None
        first_token_ms: int | None = None
        event_count = 0
        response_bytes = 0
        visible_chars = 0
        usage: dict[str, int | None] = {}
        response_model: str | None = None

        try:
            request = urllib.request.Request(
                self.endpoint,
                data=json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8"),
                headers=request_headers(
                    api_key=self.api_key,
                    auth=self.args.auth,
                    anthropic_version=self.args.anthropic_version,
                ),
                method="POST",
            )
            with urllib.request.urlopen(request, timeout=self.args.timeout_secs) as response:
                status = int(response.status)
                header_ms = ms_since(started_at, time.time())
                if self.args.stream:
                    parsed = parse_sse_response(response, started_at)
                    first_chunk_ms = parsed["first_chunk_ms"]
                    first_event_ms = parsed["first_event_ms"]
                    first_token_ms = parsed["first_token_ms"]
                    event_count = parsed["event_count"]
                    response_bytes = parsed["response_bytes"]
                    visible_chars = parsed["visible_chars"]
                    usage = parsed["usage"]
                    response_model = parsed["response_model"]
                else:
                    body = response.read()
                    response_bytes = len(body)
                    obj = json.loads(body.decode("utf-8"))
                    usage = extract_usage(obj)
                    response_model = obj.get("model") if isinstance(obj.get("model"), str) else None
                    visible_chars = len(extract_text(obj))
                ok = 200 <= status < 300
        except urllib.error.HTTPError as exc:
            status = int(exc.code)
            body = exc.read()
            response_bytes = len(body)
            error_type, error_message = parse_error_body(body)
        except (TimeoutError, socket.timeout) as exc:
            error_type = "timeout"
            error_message = str(exc)
        except (urllib.error.URLError, ssl.SSLError, OSError, json.JSONDecodeError) as exc:
            error_type = exc.__class__.__name__
            error_message = str(exc)

        completed_at = time.time()
        if not ok and not error_type:
            error_type = f"http_{status}"

        return RequestResult(
            request_index=request_index,
            stage_index=stage_index,
            stage_concurrency=stage_concurrency,
            stage_target_rpm=stage_target_rpm,
            ok=ok,
            status=status,
            error_type=error_type,
            error_message=error_message[:500],
            scheduled_at=scheduled_at,
            started_at=started_at,
            completed_at=completed_at,
            client_queue_ms=ms_since(scheduled_at, started_at),
            header_ms=header_ms,
            first_chunk_ms=first_chunk_ms,
            first_event_ms=first_event_ms,
            first_token_ms=first_token_ms,
            total_ms=ms_since(started_at, completed_at),
            event_count=event_count,
            response_bytes=response_bytes,
            visible_chars=visible_chars,
            input_tokens=usage.get("input_tokens"),
            output_tokens=usage.get("output_tokens"),
            response_model=response_model,
        )

    def next_request_index(self) -> int:
        with self.request_counter_lock:
            self.request_counter += 1
            return self.request_counter

    def write_outputs(
        self,
        results: list[RequestResult],
        stage_summaries: list[StageSummary],
        summary: dict[str, Any],
    ) -> None:
        write_csv(self.out_dir / "requests.csv", [asdict(item) for item in results])
        write_csv(self.out_dir / "stages.csv", [asdict(item) for item in stage_summaries])
        write_json(self.out_dir / "summary.json", summary)
        with (self.out_dir / "failures.jsonl").open("w", encoding="utf-8") as fh:
            for item in results:
                if not item.ok:
                    fh.write(json.dumps(asdict(item), ensure_ascii=False, separators=(",", ":")) + "\n")

    def safe_config(self) -> dict[str, Any]:
        data = vars(self.args).copy()
        data["api_key"] = mask_secret(self.api_key)
        data["endpoint"] = self.endpoint
        data["run_id"] = self.run_id
        return data

    def print_header(self) -> None:
        print("TTFT load test")
        print(f"endpoint={self.endpoint}")
        print(f"model={self.args.model}")
        if self.args.mode == "fixed":
            print(
                f"mode=fixed requests={self.args.requests} concurrency={self.args.concurrency} "
                f"stream={str(self.args.stream).lower()}"
            )
        elif self.args.mode == "ramp":
            print(
                f"mode=ramp start_concurrency={self.args.start_concurrency} "
                f"step_concurrency={self.args.step_concurrency} "
                f"max_concurrency={self.args.max_concurrency} "
                f"stage_seconds={self.args.stage_seconds:g} stream={str(self.args.stream).lower()}"
            )
        else:
            print(
                f"mode=rpm-ramp start_rpm={self.args.start_rpm:g} "
                f"step_rpm={self.args.step_rpm:g} max_rpm={self.args.max_rpm:g} "
                f"max_workers={self.args.max_workers} "
                f"stage_seconds={self.args.stage_seconds:g} stream={str(self.args.stream).lower()}"
            )
        print(f"output_dir={self.out_dir}")

    def log(self, message: str) -> None:
        with self.print_lock:
            print(time.strftime("%H:%M:%S"), message, flush=True)

    def print_stage_summary(self, stage: StageSummary) -> None:
        self.log(
            "stage={stage} target_rpm={target:g} workers={concurrency} scheduled={scheduled} "
            "completed={completed} ok={ok} completed_rpm={completed_rpm:.2f} "
            "success_rpm={rpm:.2f} success={success:.2%} queue_p95={queue}ms "
            "ttft_p95={ttft}ms total_p95={total}ms "
            "supported_concurrency_avg={avg_conc} supported_concurrency_p95={p95_conc} "
            "stable={stable} {reason}".format(
                stage=stage.stage_index,
                target=stage.target_rpm,
                concurrency=stage.concurrency,
                scheduled=stage.scheduled,
                completed=stage.completed,
                ok=stage.ok,
                completed_rpm=stage.completed_rpm,
                rpm=stage.achieved_rpm,
                success=stage.success_rate,
                queue=stage.p95_client_queue_ms,
                ttft=stage.p95_ttft_ms,
                total=stage.p95_total_ms,
                avg_conc=format_optional_float(stage.estimated_avg_supported_concurrency),
                p95_conc=format_optional_float(stage.estimated_p95_supported_concurrency),
                stable=stage.stable,
                reason=stage.stable_reason,
            )
        )


def build_payload(args: argparse.Namespace, run_id: str, request_index: int) -> dict[str, Any]:
    prompt = resolve_prompt(args, request_index)
    metadata_user_id = json.dumps(
        {
            "device_id": f"ttft-device-{request_index % max(args.device_count, 1)}",
            "account_uuid": args.account_uuid,
            "user_id": f"ttft-user-{request_index % max(args.user_count, 1)}",
            "session_id": str(uuid.uuid5(uuid.NAMESPACE_URL, f"{run_id}:{request_index}")),
        },
        separators=(",", ":"),
    )
    payload: dict[str, Any] = {
        "model": args.model,
        "max_tokens": args.max_tokens,
        "stream": args.stream,
        "messages": [{"role": "user", "content": prompt}],
        "metadata": {"user_id": metadata_user_id},
    }
    if args.system:
        payload["system"] = args.system
    if args.thinking != "off":
        payload["thinking"] = {
            "type": args.thinking,
            "budget_tokens": args.thinking_budget_tokens,
        }
    return payload


def resolve_prompt(args: argparse.Namespace, request_index: int) -> str:
    if args.prompt_file:
        template = Path(args.prompt_file).read_text(encoding="utf-8")
    else:
        template = args.prompt
    return template.format(request_index=request_index, model=args.model)


def parse_sse_response(response: Any, started_at: float) -> dict[str, Any]:
    first_chunk_ms: int | None = None
    first_event_ms: int | None = None
    first_token_ms: int | None = None
    data_lines: list[str] = []
    event_count = 0
    response_bytes = 0
    visible_chars = 0
    usage: dict[str, int | None] = {}
    response_model: str | None = None

    def finish_event(now: float) -> None:
        nonlocal first_event_ms
        nonlocal first_token_ms
        nonlocal event_count
        nonlocal visible_chars
        nonlocal response_model

        if not data_lines:
            return
        event_count += 1
        if first_event_ms is None:
            first_event_ms = ms_since(started_at, now)
        data = "\n".join(data_lines)
        if data.strip() == "[DONE]":
            return
        try:
            obj = json.loads(data)
        except json.JSONDecodeError:
            return
        merge_usage(usage, obj)
        response_model = extract_response_model(obj) or response_model
        text = extract_text(obj)
        if text:
            visible_chars += len(text)
            if first_token_ms is None:
                first_token_ms = ms_since(started_at, now)

    for raw in response:
        now = time.time()
        if first_chunk_ms is None:
            first_chunk_ms = ms_since(started_at, now)
        response_bytes += len(raw)
        line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
        if line == "":
            finish_event(now)
            data_lines = []
            continue
        if line.startswith("data:"):
            data_lines.append(line[5:].lstrip())

    if data_lines:
        finish_event(time.time())

    return {
        "first_chunk_ms": first_chunk_ms,
        "first_event_ms": first_event_ms,
        "first_token_ms": first_token_ms,
        "event_count": event_count,
        "response_bytes": response_bytes,
        "visible_chars": visible_chars,
        "usage": usage,
        "response_model": response_model,
    }


def extract_text(obj: Any) -> str:
    parts: list[str] = []
    if not isinstance(obj, dict):
        return ""

    delta = obj.get("delta")
    if isinstance(delta, dict):
        for key in ("text", "thinking", "partial_json"):
            value = delta.get(key)
            if isinstance(value, str):
                parts.append(value)

    content = obj.get("content")
    if isinstance(content, list):
        for item in content:
            if isinstance(item, dict):
                text = item.get("text")
                if isinstance(text, str):
                    parts.append(text)

    text = obj.get("text")
    if isinstance(text, str):
        parts.append(text)

    return "".join(parts)


def extract_response_model(obj: Any) -> str | None:
    if not isinstance(obj, dict):
        return None
    if isinstance(obj.get("model"), str):
        return obj["model"]
    message = obj.get("message")
    if isinstance(message, dict) and isinstance(message.get("model"), str):
        return message["model"]
    return None


def merge_usage(target: dict[str, int | None], obj: Any) -> None:
    for usage in find_usage_dicts(obj):
        for key in (
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ):
            value = int_or_none(usage.get(key))
            if value is not None:
                target[key] = value


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


def build_summary(
    args: argparse.Namespace,
    run_id: str,
    endpoint: str,
    started_wall: float,
    completed_wall: float,
    results: list[RequestResult],
    stage_summaries: list[StageSummary],
) -> dict[str, Any]:
    ok_results = [item for item in results if item.ok]
    errors = [item for item in results if not item.ok]
    wall_ms = ms_since(started_wall, completed_wall)
    status_counts: dict[str, int] = {}
    error_counts: dict[str, int] = {}
    for item in results:
        status_counts[str(item.status)] = status_counts.get(str(item.status), 0) + 1
        if not item.ok:
            key = item.error_type or f"http_{item.status}"
            error_counts[key] = error_counts.get(key, 0) + 1

    return {
        "run_id": run_id,
        "endpoint": endpoint,
        "model": args.model,
        "mode": args.mode,
        "stream": args.stream,
        "requests": len(results),
        "configured_requests": args.requests,
        "configured_concurrency": args.concurrency,
        "configured_start_rpm": args.start_rpm,
        "configured_step_rpm": args.step_rpm,
        "configured_max_rpm": args.max_rpm,
        "configured_max_workers": args.max_workers,
        "started_at": started_wall,
        "completed_at": completed_wall,
        "wall_ms": wall_ms,
        "throughput_rps": round(len(results) / max(wall_ms / 1000.0, 0.001), 4),
        "throughput_rpm": round(len(ok_results) * 60.0 / max(wall_ms / 1000.0, 0.001), 2),
        "highest_stable_concurrency": highest_stable_concurrency(stage_summaries),
        "highest_stable_target_rpm": highest_stable_target_rpm(stage_summaries),
        "highest_stable_rpm": highest_stable_rpm(stage_summaries),
        "highest_stable_supported_concurrency": highest_stable_supported_concurrency(stage_summaries),
        "stages": [asdict(item) for item in stage_summaries],
        "success": {
            "ok": len(ok_results),
            "total": len(results),
            "rate": round(safe_div(len(ok_results), len(results)), 4),
        },
        "errors": {
            "total": len(errors),
            "by_status": status_counts,
            "by_type": error_counts,
        },
        "latency_ms": {
            "ttft_first_token": describe_values(
                item.first_token_ms for item in ok_results if item.first_token_ms is not None
            ),
            "first_event": describe_values(
                item.first_event_ms for item in ok_results if item.first_event_ms is not None
            ),
            "first_chunk": describe_values(
                item.first_chunk_ms for item in ok_results if item.first_chunk_ms is not None
            ),
            "headers": describe_values(item.header_ms for item in ok_results if item.header_ms is not None),
            "total": describe_values(item.total_ms for item in ok_results),
            "client_queue": describe_values(item.client_queue_ms for item in results),
        },
        "tokens": {
            "input": describe_values(item.input_tokens for item in ok_results if item.input_tokens is not None),
            "output": describe_values(item.output_tokens for item in ok_results if item.output_tokens is not None),
        },
    }


def summarize_stage(
    stage_index: int,
    concurrency: int,
    target_rpm: float,
    scheduled: int,
    measure_seconds: float | None,
    started_at: float,
    completed_at: float,
    results: list[RequestResult],
    args: argparse.Namespace,
) -> StageSummary:
    ok_results = [item for item in results if item.ok]
    errors = len(results) - len(ok_results)
    success_rate = safe_div(len(ok_results), len(results))
    error_rate = safe_div(errors, len(results))
    wall_ms = ms_since(started_at, completed_at)
    wall_seconds = max(wall_ms / 1000.0, 0.001)
    rpm_seconds = max(measure_seconds if measure_seconds is not None else wall_seconds, 0.001)
    ttft_values = sorted(item.first_token_ms for item in ok_results if item.first_token_ms is not None)
    total_values = sorted(item.total_ms for item in ok_results)
    client_queue_values = sorted(item.client_queue_ms for item in results)
    achieved_rpm = round(len(ok_results) * 60.0 / rpm_seconds, 2)
    avg_total_ms = round(sum(total_values) / len(total_values), 2) if total_values else None
    p95_total_ms = percentile(total_values, 0.95)
    estimated_avg_concurrency = estimate_supported_concurrency(achieved_rpm, avg_total_ms)
    estimated_p95_concurrency = estimate_supported_concurrency(achieved_rpm, p95_total_ms)

    stable, stable_reason = stage_stability(
        completed=len(results),
        ok=len(ok_results),
        success_rate=success_rate,
        error_rate=error_rate,
        p95_ttft_ms=percentile(ttft_values, 0.95),
        p95_total_ms=percentile(total_values, 0.95),
        args=args,
    )

    return StageSummary(
        stage_index=stage_index,
        concurrency=concurrency,
        target_rpm=target_rpm,
        started_at=started_at,
        completed_at=completed_at,
        wall_ms=wall_ms,
        measure_seconds=round(rpm_seconds, 3),
        scheduled=scheduled,
        completed=len(results),
        ok=len(ok_results),
        errors=errors,
        success_rate=round(success_rate, 4),
        error_rate=round(error_rate, 4),
        completed_rpm=round(len(results) * 60.0 / rpm_seconds, 2),
        achieved_rpm=achieved_rpm,
        p50_ttft_ms=percentile(ttft_values, 0.50),
        p95_ttft_ms=percentile(ttft_values, 0.95),
        p99_ttft_ms=percentile(ttft_values, 0.99),
        p50_total_ms=percentile(total_values, 0.50),
        p95_total_ms=p95_total_ms,
        p99_total_ms=percentile(total_values, 0.99),
        estimated_avg_supported_concurrency=estimated_avg_concurrency,
        estimated_p95_supported_concurrency=estimated_p95_concurrency,
        p95_client_queue_ms=percentile(client_queue_values, 0.95),
        stable=stable,
        stable_reason=stable_reason,
    )


def stage_stability(
    completed: int,
    ok: int,
    success_rate: float,
    error_rate: float,
    p95_ttft_ms: int | None,
    p95_total_ms: int | None,
    args: argparse.Namespace,
) -> tuple[bool, str]:
    if completed == 0:
        return False, "no requests completed"
    if ok == 0:
        return False, "no successful requests"
    if success_rate < args.min_success_rate:
        return False, f"success_rate {success_rate:.4f} < {args.min_success_rate:.4f}"
    if error_rate > args.max_error_rate:
        return False, f"error_rate {error_rate:.4f} > {args.max_error_rate:.4f}"
    if args.p95_ttft_sla_ms > 0 and p95_ttft_ms is not None and p95_ttft_ms > args.p95_ttft_sla_ms:
        return False, f"p95_ttft_ms {p95_ttft_ms} > {args.p95_ttft_sla_ms}"
    if args.p95_total_sla_ms > 0 and p95_total_ms is not None and p95_total_ms > args.p95_total_sla_ms:
        return False, f"p95_total_ms {p95_total_ms} > {args.p95_total_sla_ms}"
    return True, "ok"


def highest_stable_concurrency(stage_summaries: list[StageSummary]) -> int:
    stable = [item.concurrency for item in stage_summaries if item.stable]
    return max(stable) if stable else 0


def highest_stable_target_rpm(stage_summaries: list[StageSummary]) -> float:
    stable = [item.target_rpm for item in stage_summaries if item.stable and item.target_rpm > 0]
    return max(stable) if stable else 0.0


def highest_stable_rpm(stage_summaries: list[StageSummary]) -> float:
    stable = [item.achieved_rpm for item in stage_summaries if item.stable]
    return max(stable) if stable else 0.0


def highest_stable_supported_concurrency(stage_summaries: list[StageSummary]) -> dict[str, Any]:
    stable = [item for item in stage_summaries if item.stable]
    if not stable:
        return {
            "stage_index": None,
            "target_rpm": 0.0,
            "success_rpm": 0.0,
            "avg": None,
            "p95": None,
        }
    best = max(stable, key=lambda item: item.target_rpm if item.target_rpm > 0 else item.achieved_rpm)
    return {
        "stage_index": best.stage_index,
        "target_rpm": best.target_rpm,
        "success_rpm": best.achieved_rpm,
        "avg": best.estimated_avg_supported_concurrency,
        "p95": best.estimated_p95_supported_concurrency,
    }


def estimate_supported_concurrency(success_rpm: float, total_latency_ms: int | float | None) -> float | None:
    if total_latency_ms is None or success_rpm <= 0:
        return None
    return round(success_rpm * (float(total_latency_ms) / 1000.0) / 60.0, 2)


def format_optional_float(value: float | None) -> str:
    if value is None:
        return "None"
    return f"{value:.2f}"


def describe_values(values_iter: Any) -> dict[str, Any]:
    values = sorted(int(value) for value in values_iter if value is not None)
    if not values:
        return {
            "count": 0,
            "min": None,
            "avg": None,
            "p50": None,
            "p90": None,
            "p95": None,
            "p99": None,
            "max": None,
        }
    return {
        "count": len(values),
        "min": values[0],
        "avg": round(sum(values) / len(values), 2),
        "p50": percentile(values, 0.50),
        "p90": percentile(values, 0.90),
        "p95": percentile(values, 0.95),
        "p99": percentile(values, 0.99),
        "max": values[-1],
    }


def percentile(sorted_values: list[int], pct: float) -> int | None:
    if not sorted_values:
        return None
    index = max(0, min(len(sorted_values) - 1, math.ceil(len(sorted_values) * pct) - 1))
    return sorted_values[index]


def print_summary(summary: dict[str, Any], out_dir: Path) -> None:
    success = summary["success"]
    errors = summary["errors"]
    latency = summary["latency_ms"]

    print()
    print("Results")
    print(
        f"ok={success['ok']}/{success['total']} success_rate={success['rate']:.2%} "
        f"errors={errors['total']}"
    )
    print(
        f"wall={summary['wall_ms']}ms throughput={summary['throughput_rps']}rps "
        f"throughput={summary['throughput_rpm']}rpm"
    )
    if summary["stages"]:
        supported = summary["highest_stable_supported_concurrency"]
        print(
            f"highest_stable_concurrency={summary['highest_stable_concurrency']} "
            f"highest_stable_target_rpm={summary['highest_stable_target_rpm']} "
            f"highest_stable_rpm={summary['highest_stable_rpm']}"
        )
        print(
            "supported_concurrency "
            f"stage={supported['stage_index']} target_rpm={supported['target_rpm']} "
            f"success_rpm={supported['success_rpm']} "
            f"avg={format_optional_float(supported['avg'])} "
            f"p95={format_optional_float(supported['p95'])}"
        )
    print_metric("ttft_first_token", latency["ttft_first_token"])
    print_metric("total", latency["total"])
    print_metric("first_event", latency["first_event"])
    print_metric("headers", latency["headers"])
    if errors["total"]:
        print(f"errors_by_status={errors['by_status']}")
        print(f"errors_by_type={errors['by_type']}")
    print(f"summary_json={out_dir / 'summary.json'}")
    print(f"requests_csv={out_dir / 'requests.csv'}")
    print(f"stages_csv={out_dir / 'stages.csv'}")
    print(f"failures_jsonl={out_dir / 'failures.jsonl'}")


def print_metric(name: str, metric: dict[str, Any]) -> None:
    if metric["count"] == 0:
        print(f"{name}: count=0")
        return
    print(
        f"{name}: count={metric['count']} avg={metric['avg']}ms p50={metric['p50']}ms "
        f"p90={metric['p90']}ms p95={metric['p95']}ms p99={metric['p99']}ms "
        f"min={metric['min']}ms max={metric['max']}ms"
    )


def normalize_endpoint(base_url: str, route: str) -> str:
    base = base_url.rstrip("/")
    if route.startswith("http://") or route.startswith("https://"):
        return route
    return base + "/" + route.lstrip("/")


def request_headers(api_key: str, auth: str, anthropic_version: str) -> dict[str, str]:
    headers = {
        "Content-Type": "application/json",
        "User-Agent": USER_AGENT,
        "anthropic-version": anthropic_version,
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


def resolve_api_key(value: str | None) -> str:
    if value:
        if value.startswith("env:"):
            name = value[4:]
            secret = os.environ.get(name)
            if not secret:
                raise SystemExit(f"missing environment variable: {name}")
            return secret
        return value
    return os.environ.get("API_KEY") or os.environ.get("KIRO_RS_API_KEY") or DEFAULT_API_KEY


def int_or_none(value: Any) -> int | None:
    try:
        if value is None:
            return None
        return int(value)
    except (TypeError, ValueError):
        return None


def ms_since(started: float, ended: float) -> int:
    return int(round((ended - started) * 1000))


def safe_div(numerator: int | float, denominator: int | float) -> float:
    if denominator == 0:
        return 0.0
    return numerator / denominator


def write_json(path: Path, data: Any) -> None:
    with path.open("w", encoding="utf-8") as fh:
        json.dump(data, fh, ensure_ascii=False, indent=2)
        fh.write("\n")


def write_csv(path: Path, rows: list[dict[str, Any]]) -> None:
    if not rows:
        path.write_text("", encoding="utf-8")
        return
    with path.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
        writer.writeheader()
        writer.writerows(rows)


def mask_secret(value: str | None) -> str | None:
    if not value:
        return value
    if len(value) <= 8:
        return "***"
    return value[:4] + "..." + value[-4:]


def env_int(name: str, default: int) -> int:
    value = os.environ.get(name)
    if not value:
        return default
    try:
        return int(value)
    except ValueError:
        raise SystemExit(f"{name} must be an integer")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Measure streaming TTFT, total latency, concurrency, and achieved RPM for /v1/messages."
    )
    parser.add_argument("--base-url", default=os.environ.get("BASE_URL", DEFAULT_BASE_URL))
    parser.add_argument("--route", default=os.environ.get("ROUTE", DEFAULT_ROUTE))
    parser.add_argument("--api-key", default=None, help="API key literal or env:NAME. Defaults to API_KEY/KIRO_RS_API_KEY.")
    parser.add_argument("--auth", choices=["x-api-key", "bearer", "both"], default=os.environ.get("AUTH", "x-api-key"))
    parser.add_argument("--anthropic-version", default=os.environ.get("ANTHROPIC_VERSION", "2023-06-01"))
    parser.add_argument("--model", default=os.environ.get("MODEL", DEFAULT_MODEL))
    parser.add_argument("--mode", choices=["rpm-ramp", "ramp", "fixed"], default=os.environ.get("MODE", "rpm-ramp"))
    parser.add_argument("-c", "--concurrency", type=int, default=env_int("CONCURRENCY", 3))
    parser.add_argument("-n", "--requests", type=int, default=env_int("REQUESTS", 12))
    parser.add_argument("--start-concurrency", type=int, default=env_int("START_CONCURRENCY", 1))
    parser.add_argument("--step-concurrency", type=int, default=env_int("STEP_CONCURRENCY", 2))
    parser.add_argument("--max-concurrency", type=int, default=env_int("MAX_CONCURRENCY", 16))
    parser.add_argument("--start-rpm", type=float, default=float(os.environ.get("START_RPM", "100")))
    parser.add_argument("--step-rpm", type=float, default=float(os.environ.get("STEP_RPM", "100")))
    parser.add_argument("--max-rpm", type=float, default=float(os.environ.get("MAX_RPM", "1000")))
    parser.add_argument("--max-workers", type=int, default=env_int("MAX_WORKERS", 128))
    parser.add_argument("--stage-seconds", type=float, default=float(os.environ.get("STAGE_SECONDS", "60")))
    parser.add_argument("--stop-on-unstable", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--min-success-rate", type=float, default=float(os.environ.get("MIN_SUCCESS_RATE", "0.95")))
    parser.add_argument("--max-error-rate", type=float, default=float(os.environ.get("MAX_ERROR_RATE", "0.05")))
    parser.add_argument("--p95-ttft-sla-ms", type=int, default=env_int("P95_TTFT_SLA_MS", 0))
    parser.add_argument("--p95-total-sla-ms", type=int, default=env_int("P95_TOTAL_SLA_MS", 0))
    parser.add_argument("--warmup-requests", type=int, default=env_int("WARMUP_REQUESTS", 0))
    parser.add_argument("--timeout-secs", type=float, default=float(os.environ.get("TIMEOUT_SECS", "180")))
    parser.add_argument("--stream", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--max-tokens", type=int, default=env_int("MAX_TOKENS", 128))
    parser.add_argument(
        "--prompt",
        default=(
            "Load test request {request_index}. Give a concise answer in one sentence "
            "about why first-token latency matters for interactive coding assistants."
        ),
        help="Prompt template. Supports {request_index} and {model}.",
    )
    parser.add_argument("--prompt-file", default=None, help="Read prompt template from a UTF-8 file.")
    parser.add_argument("--system", default=None)
    parser.add_argument("--thinking", choices=["off", "enabled"], default=os.environ.get("THINKING", "off"))
    parser.add_argument("--thinking-budget-tokens", type=int, default=env_int("THINKING_BUDGET_TOKENS", 4096))
    parser.add_argument("--device-count", type=int, default=env_int("DEVICE_COUNT", 16))
    parser.add_argument("--user-count", type=int, default=env_int("USER_COUNT", 128))
    parser.add_argument("--account-uuid", default=os.environ.get("ACCOUNT_UUID", "ttft-load-test"))
    parser.add_argument("--run-id", default=None)
    parser.add_argument("--out-dir", default=None)
    parser.add_argument("--progress-every", type=int, default=env_int("PROGRESS_EVERY", 10))
    parser.add_argument("--fail-on-error", action="store_true")
    return parser


def validate_args(args: argparse.Namespace) -> None:
    if args.concurrency < 1:
        raise SystemExit("--concurrency must be >= 1")
    if args.requests < 1:
        raise SystemExit("--requests must be >= 1")
    if args.start_concurrency < 1:
        raise SystemExit("--start-concurrency must be >= 1")
    if args.step_concurrency < 1:
        raise SystemExit("--step-concurrency must be >= 1")
    if args.max_concurrency < args.start_concurrency:
        raise SystemExit("--max-concurrency must be >= --start-concurrency")
    if args.start_rpm <= 0:
        raise SystemExit("--start-rpm must be > 0")
    if args.step_rpm <= 0:
        raise SystemExit("--step-rpm must be > 0")
    if args.max_rpm < args.start_rpm:
        raise SystemExit("--max-rpm must be >= --start-rpm")
    if args.max_workers < 1:
        raise SystemExit("--max-workers must be >= 1")
    if args.stage_seconds <= 0:
        raise SystemExit("--stage-seconds must be > 0")
    if not 0 <= args.min_success_rate <= 1:
        raise SystemExit("--min-success-rate must be between 0 and 1")
    if not 0 <= args.max_error_rate <= 1:
        raise SystemExit("--max-error-rate must be between 0 and 1")
    if args.p95_ttft_sla_ms < 0:
        raise SystemExit("--p95-ttft-sla-ms must be >= 0")
    if args.p95_total_sla_ms < 0:
        raise SystemExit("--p95-total-sla-ms must be >= 0")
    if args.warmup_requests < 0:
        raise SystemExit("--warmup-requests must be >= 0")
    if args.max_tokens < 1:
        raise SystemExit("--max-tokens must be >= 1")
    if args.timeout_secs <= 0:
        raise SystemExit("--timeout-secs must be > 0")


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    validate_args(args)
    return TtftLoadTester(args).run()


if __name__ == "__main__":
    sys.exit(main())
