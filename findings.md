# Findings

## Current Task Findings
- SQLite store and runtime settings modules are present under `src/kiro/store.rs` and `src/kiro/settings.rs`.
- Admin runtime settings and policy routes are registered in `src/admin/router.rs`.
- Admin UI now has table-oriented components in `admin-ui/src/components/account-table.tsx`, `runtime-settings-dialog.tsx`, and `policy-dialog.tsx`.
- Initial frontend production build passes.

## Watch Items
- Runtime settings from Admin are applied immediately to runtime status and limiter state.
- DB bootstrap no longer repeats `credentials.json` import when SQLite already has credentials.
- Token/stat persistence paths now write full credential snapshots to SQLite when a store is configured.
- Docker dev uses the mounted `config/kiro-rs.db` SQLite file; WAL/SHM files are also created in `config/`.
- Frontend build passes after the table/dialog changes.

## Residual Risks
- Browser-level interaction was smoke-tested via HTTP/static build, not through a visual Playwright/in-app browser run in this turn.
- The richer account detail drawer/edit form for endpoint/proxy/备注 remains a follow-up if needed; the shipped UI focuses on table management and runtime/policy controls.

## Session Affinity Findings
- Existing converter already extracts `metadata.user_id` session UUID into Kiro `conversationId`; that value can also be reused as the affinity key.
- Token manager dispatch currently accepts only model filtering, so affinity requires threading an optional session id through handlers/provider/acquire_context.
- Priority mode previously preferred `current_id`; affinity is now checked before `current_id` so a known session can stay on its bound account when dispatchable.
- Bindings are removed on token acquisition failure to avoid pinning a session to a broken credential.
- Runtime output now exposes total and per-account affinity binding counts; a real `/v1/messages` call with stable `metadata.user_id` produced `sessionAffinityBindings: 1`.
- Production runbook now documents that request responses are not cached, and that cache locality depends on keeping `metadata.user_id` stable per conversation.

## Virtual Cache Usage Findings
- Anthropic prompt caching uses `cache_control` with ephemeral TTL values `5m` and `1h`; usage responses include cache read tokens, cache creation tokens, and 5m/1h cache creation buckets.
- The local implementation is synthetic accounting only. It does not cache responses and does not prove upstream Kiro billed the same cache amounts.
- The in-memory ledger key includes credential id, model, and session key. This prevents cross-account/model/session leakage while still letting repeated no-metadata tests use the configured fallback model bucket.
- Normal `/v1/messages` streaming can only use estimated input tokens in `message_start`; `/cc/v1/messages` buffered streaming can revise `message_start.usage` after the upstream `contextUsageEvent`.
- Streaming usage now previews before `message_start` and commits only on normal stream completion, so upstream stream errors do not inflate later `cache_read_input_tokens`.

## Medium Rate-Limit Dispatch Findings
- Recent live logs showed Opus 4.7 returning `429` with `reason=INSUFFICIENT_MODEL_CAPACITY`, while Sonnet/Opus 4.6 worked immediately on the same account. That points to model-capacity pressure, not account-wide throttling.
- Account cooldown is still appropriate for ordinary 429 responses, but model-capacity 429 should not remove the account from other models.
- Request-local failover needs an excluded credential set so the same failed account is not retried again within one request.
- Session affinity must be ignored when the bound credential is in the request-local excluded set, otherwise failover can stick to the account that just failed.
- If every candidate account hits `INSUFFICIENT_MODEL_CAPACITY`, a short model-level cooldown avoids hammering that same model while leaving other models available.
- Provider retry loops must break when no non-excluded replacement account is dispatchable; otherwise a single-account pool can spin after the only account is excluded.

## Token Auto Refresh Findings
- Existing request path already refreshes tokens lazily when they are expired or close to expiry, but that can add latency to the first request that lands on a stale credential.
- A background scheduler is useful for production because it moves refresh latency outside user requests while keeping the request-time refresh fallback.
- The scheduler should skip API Key credentials because they do not use refreshToken.
- Reading runtime settings inside every scheduler loop lets Admin changes take effect without restart.

## Dynamic Virtual Usage Findings
- The previous virtual cache mode intentionally reported `input_tokens` from `virtualCacheUncachedInputTokens`, so cctest-like repeated calls often showed `input_tokens: 1`.
- Later-turn cache creation often stayed at `virtualCacheMinCreationTokens` because the observed context delta can be small, so the configured floor dominated.
- A safer natural mode is configurable rather than forced: estimate ordinary input from the latest user message, then clamp it; vary cache creation from context delta, output size, deterministic jitter, and optional burst turns.
- The deterministic jitter uses stable hashing instead of random state so tests remain reproducible and the in-memory ledger remains simple.

## Dynamic Proxy Findings
- WindsurfAPI's dynamic proxy implementation is a binding state machine, not just an extra proxy field: generate provider credentials, verify egress IP, persist active/failed/expired status, renew expiring bindings, and rotate failed bindings.
- `kiro.rs` already has manual per-account proxy fields (`proxy_url`, `proxy_username`, `proxy_password`) plus global proxy fallback. Token refresh and Kiro API calls already compute an effective proxy.
- The missing piece in `kiro.rs` is dynamic binding persistence and lifecycle: SQLite binding table, runtime settings, Rust proxy verifier, background worker, Admin actions, and proxy-error-triggered rotation.
- Effective proxy order should become dynamic active binding > manual account proxy > global proxy, with existing `direct` manual proxy still bypassing global proxy when no dynamic active binding exists.
- Dynamic proxy helps account/IP isolation and proxy failure recovery; it should not be presented as a fix for upstream model-capacity errors such as `INSUFFICIENT_MODEL_CAPACITY`.

## Opus 4.7 Latency Investigation
- Verified by unit test: identical Anthropic request converted to Opus 4.6 vs 4.7 has the same Kiro request structure after normalizing modelId/continuation id. Plain claude-opus-4-7 does not enable thinking.
- Public Kiro changelog says Claude Opus 4.7 uses model id claude-opus-4.7 and adaptive thinking needs Kiro IDE 0.11.133+ / CLI 2.2.0+ for best performance and efficiency. Existing config/default used KiroIDE 0.11.107, which is a credible cause of poor 4.7 behavior.
- Updated default/local kiroVersion to 0.12.155 and added requestDiagnosticsEnabled for safe upstream request summary logging.

## Opus 4.7 Stream Latency Follow-up
- User provided stream=true samples after Kiro version upgrade: status=200, attempts=1, queue/acquire 0, but upstream_ms varied 2.9s, 3.5s, 12.6s, 14.2s for max_tokens=16. Need compare KAM request shape and public info.
- Public Kiro docs/changelog confirm Opus 4.7 is still marked experimental, limited to us-east-1/eu-central-1 and IDC auth, and adaptive thinking needs Kiro IDE 0.11.133+ / CLI 2.2.0+ for best performance. That explains some natural variance, but does not rule out local request/client issues.
- Compared with KAM: Opus 4.7 model id remains `claude-opus-4.7`, so the current model mapping is not the likely latency bug.
- Compared with KAM: KAM uses a stable `agentContinuationId = conversationId`, while `kiro.rs` currently generates a fresh random `agentContinuationId` for every request. This can reduce upstream session/cache locality and is a plausible latency/consistency issue.
- Compared with KAM: KAM tunes reqwest pooling/keepalive, while `kiro.rs` sets `Connection: close` on Kiro API requests. That defeats connection reuse and is a plausible cause of stream variability.
- Current `upstream_ms` for stream=true includes full response body consumption; it does not separate time-to-first-upstream-chunk or time-to-first-decoded-event. Need add first-chunk/first-event logs before drawing conclusions from 12-14s totals.
- Fix applied: `agentContinuationId` now equals `conversationId`; reqwest clients now use KAM-style pooling/keepalive; provider no longer sends `Connection: close` for upstream Kiro API/MCP requests.
- New logs to read after a successful stream: `upstream_stream_first_chunk`, `upstream_stream_first_event`, and final `upstream_request_timing`. If first chunk/event is fast but final upstream_ms is high, UX latency is not initial response latency. If first chunk/event is high, the delay is upstream/model/account/region.
