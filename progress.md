# Progress

## Session Log
- Resumed from an existing implementation handoff.
- Confirmed new SQLite/runtime/Admin files and routes exist.
- Ran `pnpm --dir admin-ui build`: passed.
- Updated planning files to track the current Admin table + SQLite policy task.
- Fixed startup behavior so SQLite-backed deployments can boot even if `credentials.json` is unavailable.
- Fixed global RPM accounting so queue-full/queue-timeout paths do not consume RPM slots before a request actually gets a global execution permit.
- Fixed Admin-added credentials to preserve the selected `endpoint`.
- Added tests for SQLite runtime settings persistence, one-time JSON import, endpoint preservation, and queue-full RPM accounting.
- Built and started `docker-compose-dev.yml`; verified container health and mounted SQLite files.
- Started soft session affinity implementation for better upstream cache locality.
- Added in-memory session affinity in `MultiTokenManager` with a 12-hour TTL.
- Threaded session affinity keys through Anthropic conversion, standard `/v1/messages`, `/cc/v1/messages`, and WebSearch MCP calls.
- Added Admin runtime fields for total/per-account session affinity binding counts and displayed the total in Admin UI.
- Added tests for same-session reuse, rebind on full account, and conversion affinity key extraction.
- Documented production cache/session-affinity usage in `docs/production-single-node.md`.
- Rebuilt the dev Docker image after session-affinity changes and verified the running container.

## Validation
- `cargo check`: passed.
- `cargo test`: passed, 212 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: image build passed; first start hit occupied port 8990 from old `kiro-rs-prod`; after stopping that container, `docker compose -f docker-compose-dev.yml up -d` passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed again after session-affinity changes.
- `docker compose -f docker-compose-dev.yml ps`: `kiro-rs-dev` is healthy.
- `GET /healthz`: 200.
- `GET /readyz`: 200 with current local credential.
- `GET /api/admin/settings/runtime`: 200, returns DB-backed runtime settings.
- `GET /api/admin/runtime`: 200, returns `sessionAffinityBindings`.
- `PUT /api/admin/settings/runtime`: hot update observed in `/api/admin/runtime`, then restored.
- `PATCH /api/admin/credentials/1/policy`: policy override and restore verified in `/api/admin/runtime`.
- `GET /admin`: 200 and returns built React HTML.
- Real `/v1/messages` smoke with stable `metadata.user_id`: 200; runtime showed `sessionAffinityBindings: 1` and `inFlight: 0` after completion.

## Residual Scope
- The table UI, runtime settings dialog, policy dialog, filters, sorting, pagination, column visibility, batch policy, cooldown clearing, import/export, and batch enable/disable flows are implemented.
- A full right-side account detail drawer for editing endpoint/proxy/备注 as a richer form is not yet implemented as a dedicated feature; current policy editing covers concurrency/RPM overrides, while endpoint/proxy are still managed through add/import/export paths.

## Completed: Session Affinity
- Stable session IDs are extracted during Anthropic conversion, passed to provider/token manager, and used to prefer the previously bound credential when dispatchable.
- Runtime state is in-memory `session_id -> credential_id` with a 12-hour TTL and Admin runtime counters.

## In Progress: Virtual Cache Usage
- Completed: return Anthropic-compatible cache usage fields for downstream new-api/cctest accounting.
- Confirmed current main `/v1/messages` usage only returns `input_tokens` and `output_tokens`; WebSearch already emits zero cache fields but is outside this virtual usage flow.
- Implementation approach: runtime-configured in-memory ledger keyed by credential/model/session with separate 5m and 1h buckets.
- Added request parsing for `cache_control.ttl` on tools, system blocks, and message content blocks.
- Added virtual cache runtime settings persisted in SQLite and exposed in the Admin runtime settings dialog.
- Added preview/commit accounting so streaming responses can emit `message_start.usage` without committing interrupted/error streams to the ledger.
- Re-ran `cargo fmt`, `cargo check`, `cargo test`, `pnpm --dir admin-ui build`, and `docker compose -f docker-compose-dev.yml up -d --build`.
- Verified `GET /healthz`, `GET /readyz`, and `GET /api/admin/settings/runtime`.
- Real `/v1/messages` smoke, same `metadata.user_id`: first response returned `input_tokens: 1`, `cache_read_input_tokens: 0`, `cache_creation_input_tokens: 18000`; second response returned `cache_read_input_tokens: 18000`, `cache_creation_input_tokens: 128`.
- Real `/v1/messages` smoke with `cache_control.ttl: "1h"` returned creation in `cache_creation.ephemeral_1h_input_tokens` and zero 5m creation.
- Real `/cc/v1/messages` streaming smoke returned full cache usage fields in `message_start.message.usage`.

## Latest Validation
- `cargo check`: passed.
- `cargo test`: passed, 219 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed.
- `docker compose -f docker-compose-dev.yml ps`: `kiro-rs-dev` is healthy.

## In Progress: Medium Rate-Limit Dispatch
- Added runtime settings for `maxRetryAccounts` and `modelCapacityCooldownMs`.
- Added in-memory model cooldown manager with Admin runtime snapshot support.
- Added upstream error classification for HTTP status, JSON `reason`, and `Retry-After` seconds/date parsing.
- Changed `/v1/messages` and `/cc/v1/messages` provider flow to maintain a request-local excluded credential set and try up to the configured number of different accounts.
- `INSUFFICIENT_MODEL_CAPACITY` no longer sets account cooldown; if all attempted accounts hit model capacity for the same model, the model enters short cooldown.
- Other 429 responses still apply account cooldown, using `Retry-After` when present.
- 408/5xx/network failures apply transient account cooldown and can fail over to another account.
- Fixed a retry-loop edge case where no dispatchable replacement account could cause repeated acquire attempts.
- Added tests for excluded session-affinity dispatch, all-excluded acquire failure, `Retry-After` seconds/date parsing, upstream reason extraction, and model cooldown expiry.

## Latest Validation: Medium Rate-Limit Dispatch
- `cargo fmt -- --check`: passed after formatting.
- `cargo check`: passed.
- `cargo test`: passed, 226 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed; `kiro-rs-dev` healthy.
- `GET /healthz`: 200.
- `GET /api/admin/settings/runtime`: returns `maxRetryAccounts: 3`, `modelCapacityCooldownMs: 10000`.
- `GET /api/admin/runtime`: returns `modelCooldowns` and the new runtime fields.
- Local dev DB `rateLimitCooldownMs` was restored from temporary diagnostic `0` to `60000`.

## Completed: Token Auto Refresh Scheduler
- Added runtime settings:
  - `tokenAutoRefreshEnabled` default `true`
  - `tokenAutoRefreshIntervalSecs` default `300`
  - `tokenAutoRefreshWindowSecs` default `1800`
- Persisted the new settings in SQLite and exposed them through Admin runtime settings/status.
- Added a background Tokio task that reads the latest runtime settings each loop, scans refreshable Social/IdC credentials, and force-refreshes tokens expiring inside the configured window.
- API Key credentials are skipped by the scheduler.
- Admin runtime settings dialog now exposes the auto-refresh switch and timing fields.

## Latest Validation: Token Auto Refresh Scheduler
- `cargo check`: passed.
- `cargo test`: passed, 227 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed; `kiro-rs-dev` healthy.
- `GET /healthz`: 200.
- `GET /api/admin/settings/runtime`: returns `tokenAutoRefreshEnabled: true`, `tokenAutoRefreshIntervalSecs: 300`, `tokenAutoRefreshWindowSecs: 1800`.
- `GET /api/admin/runtime`: returns the same auto-refresh runtime fields.

## Completed: Dynamic Virtual Cache Usage
- Added runtime settings for fixed vs estimated latest-user input tokens and fixed vs dynamic cache creation tokens.
- Added SQLite persistence keys and Admin runtime settings controls for the new virtual usage modes.
- Implemented deterministic dynamic cache creation using context delta, output size, jitter, and optional burst writes.
- Threaded latest user input token estimates into non-stream, normal stream, and buffered `/cc/v1/messages` usage builders.

## Latest Validation: Dynamic Virtual Cache Usage
- `cargo check`: passed.
- `cargo test`: passed, 230 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed; `kiro-rs-dev` healthy.
- `GET /healthz`: 200.
- `GET /api/admin/settings/runtime`: returns the new virtual cache dynamic mode fields.
- `PUT /api/admin/settings/runtime`: hot update to `estimated_user_delta` + `dynamic` succeeded, then local settings were restored to the previous fixed mode.

## In Progress: Dynamic Proxy/IP Binding
- Inspected WindsurfAPI `dynamic-proxy.js`, `proxy-test.js`, `proxy-config.js`, and availability worker integration.
- Confirmed `kiro.rs` already supports manual account proxy/global proxy and effective proxy usage in both token refresh and Kiro provider client selection.
- Started implementing a Rust-native dynamic proxy layer rather than copying JS code directly.
- Added dynamic proxy runtime settings, SQLite binding table, Rust dynamic proxy manager/verifier, and background maintenance worker.
- Wired effective proxy lookup into token refresh, usage-limit lookup, MCP calls, and normal Kiro API calls.
- Added Admin API/UI controls for single-account and batch bind/rotate/verify/clear operations.

## Latest Validation: Dynamic Proxy/IP Binding
- `cargo check`: passed.
- `cargo test`: passed, 233 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed.
- `GET /healthz`: 200.
- `GET /api/admin/runtime`: returns `dynamicProxy` summary.
- `GET /api/admin/settings/runtime`: returns dynamic proxy configuration fields.
- `docker compose -f docker-compose-dev.yml ps`: `kiro-rs-dev` healthy.

## Completed: Opus 4.7 Latency Investigation
- Updated default and local config Kiro version from 0.11.107 to 0.12.155.
- Added safe request diagnostics logging behind config requestDiagnosticsEnabled. Local config enables it for comparison.
- Added tests proving 4.6/4.7 conversion differs only by modelId/continuation id.
- Validation: cargo fmt -- --check, cargo check, cargo test, pnpm --dir admin-ui build all passed.

## In Progress: Opus 4.7 Stream Latency Follow-up
- Started comparing kiro.rs with sibling kiro-account-manager and public Kiro/Opus 4.7 information.
- Found two local differences likely to affect stream stability: `Connection: close` on upstream Kiro requests, and random per-request `agentContinuationId`.
- Next change: enable HTTP connection reuse/keepalive, stabilize `agentContinuationId`, and add first upstream chunk/event timing logs.
- Implemented KAM-style HTTP client keepalive/pool settings and removed `Connection: close` from Kiro API/MCP upstream requests.
- Changed `agentContinuationId` to equal `conversationId`, matching KAM behavior and improving upstream session continuity.
- Added stream diagnostics:
  - `upstream_stream_first_chunk` records first upstream body chunk timing.
  - `upstream_stream_first_event` records first decoded AWS event timing.
- Local smoke with current dev credentials returned `INVALID_MODEL_ID` for Opus 4.7/4.6, so local accounts cannot validate real Opus latency. Diagnostics confirmed request model id, version, no thinking directive, and API region.

## Latest Validation: Opus 4.7 Stream Latency Follow-up
- `cargo fmt -- --check`: passed.
- `cargo check`: passed.
- `cargo test`: passed, 235 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed.
- `docker compose -f docker-compose-dev.yml ps`: `kiro-rs-dev` healthy.
- `GET /healthz`: 200.

## Completed: Configurable Session Affinity TTL
- Replaced the hardcoded 12-hour session affinity TTL with runtime setting `sessionAffinityTtlSecs`.
- Default TTL is now 3600 seconds, with validation range `300..43200`.
- Persisted the setting in SQLite runtime settings and exposed it through Admin settings/status APIs.
- Admin runtime settings dialog now includes `会话亲和 TTL 秒数`.
- New affinity binding and renewal both use the latest runtime TTL; existing bindings naturally expire or renew under the new setting.

## Latest Validation: Configurable Session Affinity TTL
- `cargo fmt -- --check`: passed.
- `cargo check`: passed.
- `cargo test`: passed, 236 tests.
- `pnpm --dir admin-ui build`: passed.
- `docker compose -f docker-compose-dev.yml up -d --build`: passed.
- `docker compose -f docker-compose-dev.yml ps`: `kiro-rs-dev` healthy.
- `GET /healthz`: 200.
- `GET /api/admin/settings/runtime`: returned `sessionAffinityTtlSecs: 3600`.
- `PUT /api/admin/settings/runtime`: hot update to `900` succeeded, then local setting was restored to `3600`.

## Completed: Opus 4.7 Detector Failure Analysis
- User provided cctest/hvoy results showing current `claude-opus-4-7` now passes identity/protocol/message-id checks on hvoy, but still partially fails model signature and fails PDF document recognition plus structured output.
- Started comparing local gateway behavior with sibling `kiro-account-manager`, public Kiro proxy implementations, and official Anthropic protocol details.
- Findings were recorded in `findings.md`; no business logic was changed in this analysis turn.

## Completed: Opus 4.7 Git Regression Check
- Inspected commits `3f9e229`, `1b070bb`, and current `33df672` around `reasoningContentEvent`, `thinking_enabled`, and plain Opus 4.7 stabilization.
- Found that `3f9e229` initially passed Kiro reasoning/signature through when present, while `1b070bb` gated it behind `thinking_enabled`.
- Confirmed current plain `claude-opus-4-7` hard-forces `client_thinking_enabled=false`, so upstream signatures seen in diagnostics are intentionally hidden from the client.
- Recorded the corrected hypothesis in `findings.md`: the likely main issue is signature/signed-thinking preservation and exposure policy, not adaptive stabilization alone.

## Completed: Stable Proxy Comparison
- Read `tmp/stable_opus47.env` without printing secrets and used it to run structure-only probes against the stable reverse-Kiro endpoint.
- Compared plain stream, adaptive stream, non-stream, `-thinking` model, concurrent 64k stream probes, `/v1/models`, and WebSearch shape.
- Found the stable endpoint passes through only text for plain/adaptive/`-thinking` 4.7; it does not expose `thinking_delta` or `signature_delta`.
- WebSearch shape matched local closely, so WebSearch is unlikely to explain the cctest delta.
- Recorded the updated finding: cctest "signature" should not be interpreted only as Anthropic extended-thinking `signature_delta`; broader envelope/usage/model-list fingerprints are likely involved.

## Completed: CCTest Compat Switches
- Added runtime/Admin switches:
  - `compatUsageShape`: `anthropic` or `flat`
  - `compatThinkingModel`: `native` or `plain_text`
  - `compatModelsShape`: `anthropic` or `aggregator`
- `flat` usage removes nested `cache_creation` while keeping top-level cache read/create fields.
- `plain_text` makes Opus 4.7 `-thinking` responses hide thinking/signature and report response model `claude-opus-4-7`.
- `aggregator` changes `/v1/models` entries toward the stable proxy shape with `type: "model"`, `owned_by: null`, and `max_tokens: null`.
- Validation: `cargo fmt -- --check`, `cargo check`, `cargo test` passed with 245 tests; `pnpm --dir admin-ui build` passed.

## Completed: ANTML Probe Sibling/Public Research
- Checked `/Users/zhangyu/code/myProject/supertoken-projects/kiro-account-manager` for `antml`, `cctest`, short refusal, Opus 4.7, reasoning/signature, and retry-related handling. Found no exact ANTML/CCTest probe compatibility path.
- Checked `/Users/zhangyu/code/myProject/supertoken-projects/WindsurfAPI`; found no exact ANTML probe handling, but found relevant patterns for narrow prompt rewriting, anti-refusal hints, opt-in retry-with-correction, and policy-block short-circuiting.
- Searched public web/GitHub for `antml` + `cctest` / `I can't discuss that`; found ANTML/XML transport issues in Claude Code but no standard discussion or workaround for this exact tag probe.
- Recorded details in `findings.md`; no business logic was changed in this research turn.

## Completed: telagod/llm-probe Review
- Cloned `https://github.com/telagod/llm-probe` to `/tmp/llm-probe-inspect` for read-only inspection.
- Confirmed it has no exact `antml`/Chinese tag prompt probe.
- Found useful diagnostics patterns in `authenticity` (cross-run consistency signatures and drift score), `stream` (strict Anthropic SSE contract validation), and `injection` (random sentinel leak/hidden-tool hard gates).
- Recorded the takeaway in `findings.md`: useful for measuring/diagnosing Opus 4.7 instability, but not a ready-made workaround.

## Completed: Opus 4.7 ANTML Probe Compat Switch
- Added runtime/Admin setting `opus47AntmlProbeCompat` with values `off` and `clarify`; default remains `off`.
- The clarify mode only applies to plain `claude-opus-4-7` / `claude-opus-4.7` when the current user content matches the cctest-style ANTML probe shape.
- Matching requests get a short clarification prepended to the Kiro upstream current user message; no response spoofing or retry was added.
- Added unit coverage for disabled mode, matched plain Opus 4.7 probe, non-probe text, and thinking model exclusion.
- Validation: `cargo fmt -- --check`, `cargo check`, `cargo test` passed with 249 tests; `pnpm --dir admin-ui build` passed.

## Completed: PDF / Structured Output Follow-up
- Added Anthropic `document` block handling for `application/pdf`: base64 PDF text is extracted and appended to the current user content.
- Added `pdf-extract` plus a lightweight `Tj`/`TJ` fallback for simple text-layer PDFs.
- Added structured-output compatibility for `output_config.format` and OpenAI-style `response_format`, injecting request-scoped JSON-only/schema instructions.
- Removed Kiro `documents` field and assistant `reasoningContent` history emission after live logs showed upstream `400 Improperly formed request`; PDF remains text-only on the Kiro request.
- Added converter tests for PDF extraction, JSON schema hinting, and OpenAI `response_format=json_object`.
- Validation after rollback of unsupported fields: `cargo fmt -- --check` passed; `cargo test` passed with 252 tests; `pnpm --dir admin-ui build` passed.
- Follow-up after live logs showed `extracted_chars=8`: enhanced PDF fallback to inspect PDF streams, inflate `/FlateDecode` streams, decode literal and hex text strings, and detect UTF-16BE hex strings without BOM.
- Validation after fallback enhancement: `cargo fmt -- --check` passed; `cargo test` passed with 253 tests; `pnpm --dir admin-ui build` passed.

## Completed: Opus 4.7 Clean Probe Mode
- Added runtime config field `opus47CleanProbeMode` and persisted/Admin-exposed values `off` / `clean`.
- Added Admin UI runtime-settings dropdown `Opus 4.7 Clean Probe`.
- Added `ConversionOptions.clean_probe_mode` and wired `/v1/messages` plus `/cc/v1/messages` conversion through it.
- Clean mode is scoped to plain Opus 4.7 and avoids local synthetic prompt/history/tool-description additions that can pollute detector probes.
- Added diagnostics so `opus47_request_thinking_state` logs `clean_probe_mode`, making live cctest/hvoy comparisons explicit.
- Added config example and README entries for the Opus 4.7 detector/diagnostic settings.
- Validation: `cargo fmt -- --check`, `cargo test clean_probe -- --nocapture`, `cargo test`, `pnpm --dir admin-ui exec tsc --noEmit`, and `pnpm --dir admin-ui build` passed.
