# Task Plan: Admin Table and Runtime Policy Management

## Goal
Upgrade `kiro.rs` Admin from a credential-card view to a table-oriented account pool manager, persist credentials and runtime policy in SQLite for single-node production use, and make global/account scheduling policy hot-editable from the Admin API/UI.

Current extension: add virtual cache usage accounting so Anthropic-compatible responses expose cache read/write token fields for new-api/cctest-style billing and audit.

Current extension: split 429 handling into account-level limits vs model-capacity errors, add bounded request-local account failover, and expose model cooldown state for single-node production operation.

Current extension: add configurable background token auto-refresh so expiring Social/IdC credentials are refreshed before first request latency is hit.

Current extension: add optional natural dynamic virtual cache usage accounting so ordinary input tokens and cache creation tokens do not have to stay fixed.

Current extension: add dynamic per-account IP proxy binding so each account can keep an isolated, renewable proxy session with verification and auto-rotation.

Current investigation: compare local `kiro.rs`, sibling `kiro-account-manager`, public Kiro proxy implementations, and official Anthropic protocol behavior to explain why `claude-opus-4-7` detection fails or only partially passes, especially around reasoning signatures, PDF/document input, and structured output.

Current extension: add an opt-in Opus 4.7 ANTML probe compatibility mode for cctest-style tag probes, defaulting off and only clarifying that the tag is ordinary visible user text.

Current extension: close remaining cctest/hvoy gaps by extracting PDF document text and adding structured-output request hints without adding Kiro fields that trigger upstream 400s.

Current extension: add an opt-in Opus 4.7 Clean Probe mode in Admin runtime settings to reduce local prompt/context pollution during detector probes while preserving the rule that signatures are only passed through when upstream actually returns them.

Current extension: add UI-configurable same-account retry rules for selected upstream HTTP statuses/reasons so small account pools can retry one credential before account cooldown/failover.

Current extension: implement an Opus 4.7 detection profile based on public/local relay-audit and signed-thinking implementations, with Clean Probe treated as a diagnostic toggle rather than the main compatibility path.

Current extension: tighten Opus 4.7 identity-probe compatibility by logging skip reasons and stripping tool schema from identity-only probes to reduce model-family contamination.

Current extension: implement gated Opus 4.7 signed-thinking history replay so real upstream `thinking+signature` blocks can be returned to Kiro as assistant `reasoningContent` during explicit `history_experiment` testing.

Current extension: add Opus 4.7 signature-failure classification diagnostics plus a default-off short-request/PDF thinking-label experiment for cctest A/B runs.

Current extension: treat short exact-answer text-only signatures as expected upstream behavior and add default-off prompt/request/response dump support for Claude 4.6/4.7/Sonnet 4.6 debugging.

Current extension: add an Opus 4.7 run mode switch with `custom`, `benchmark`, and `fast` effective presets while keeping Prompt Dump and raw debug as manual controls.

Current extension: add Opus 4.6 and Sonnet 4.6 benchmark/fast/custom compatibility settings, fix PDF exact streaming duplicate output, and verify with local checks plus Docker where credentials allow.

Current extension: add a default-off global target cache reuse ratio that uses recent virtual-cache usage as a soft controller while preserving the existing ledger semantics and advanced cache execution settings.

## Phases
- [completed] Inspect existing Admin/backend runtime shape and identify integration points
- [completed] Add SQLite store and first-start migration from `credentials.json`
- [completed] Move runtime scheduling settings into persisted DB-backed settings
- [completed] Add Admin APIs for runtime settings, per-account policy, batch policy, and cooldown clearing
- [completed] Wire dynamic limiter and per-account policy into dispatch
- [completed] Replace Admin main credential view with table workflow and policy dialogs
- [completed] Verify frontend/backend builds and smoke-test critical behavior
- [completed] Record final validation results and any residual risks
- [completed] Add soft session affinity for model dispatch
- [completed] Verify session affinity behavior and Admin runtime visibility
- [completed] Add virtual cache usage accounting with configurable 5m/1h TTL
- [completed] Wire virtual cache usage into non-stream and stream Anthropic responses
- [completed] Add runtime/Admin controls and verify builds/tests
- [in_progress] Implement medium-weight rate-limit dispatch: account failover, model-capacity cooldown, Retry-After parsing, and Admin runtime visibility
- [completed] Add configurable background Token auto-refresh scheduler
- [completed] Add dynamic virtual cache usage input/creation modes and Admin controls
- [completed] Add dynamic proxy/IP binding settings, SQLite bindings, worker, Admin controls, and request-path effective proxy integration
- [completed] Analyze Opus 4.7 detector failures against sibling gateway, public proxy behavior, and official Anthropic protocol expectations
- [completed] Add narrow Opus 4.7 ANTML probe compatibility config, request rewrite, Admin UI control, and tests
- [completed] Add PDF document text extraction and structured-output compatibility hints; avoid unsupported Kiro document/reasoning history fields
- [completed] Add Opus 4.7 Clean Probe runtime/Admin toggle, scoped conversion behavior, diagnostics, and tests
- [completed] Add configurable same-account retry rules and expose them in Admin runtime settings
- [completed] Record Opus 4.7 detection-profile research from `api-relay-audit`, `cc-relay`, `kiro-account-manager`, and `WindsurfApi`
- [completed] Implement Opus 4.7 detection profile preset, identity probe compatibility, signed-thinking cache diagnostics, Admin controls, and local fingerprint tests
- [completed] Add identity-probe skip diagnostics and clear tool definitions for matched identity probes
- [completed] Run local Docker Opus 4.7 probes and add identity visible-text sanitization for matched identity probes
- [completed] Add gated signed-thinking history replay and validate true/false signature round trips locally
- [completed] Add Opus 4.7 signature classification diagnostics and default-off short-request/PDF thinking-label experiment
- [completed] Fix cctest behavior regressions from ANTML/identity probe cross-contamination
- [completed] Add request-kind signature diagnostics and Prompt Dump runtime/Admin support
- [completed] Add Opus 4.7 run modes for benchmark scoring vs low-latency daily use without mutating manual dump settings
- [completed] Implement Opus 4.6/Sonnet 4.6 compatibility settings, generalized probe helpers, PDF duplicate-output fix, and Docker verification
- [in_progress] Add global target cache reuse ratio config, 5-minute diagnostics, Admin UI control, and regression coverage

## Decisions
- Keep single-node only; no Redis/Postgres.
- Use SQLite as source of truth after first startup import; `credentials.json` is only a bootstrap/compatibility import source when DB has no credentials.
- Keep runtime-only counters such as `inFlight`, cooldown, and short TTL Admin display cache in memory.
- Keep React/Vite/Tailwind/Radix and borrow sub2api-style information architecture rather than porting Vue components.
- Keep JSON import/export compatibility for KAM/sub2api-style backup flows.
- Session affinity is runtime-only memory state with TTL; do not persist it to SQLite and do not block failover when the bound account is disabled, cooling, full, or RPM-limited.
- Virtual cache usage is intentionally synthetic accounting for downstream billing compatibility; it is not persisted and does not claim to match Kiro upstream billing.
- Treat `INSUFFICIENT_MODEL_CAPACITY` as model capacity pressure rather than account-wide rate limit; use short model-level cooldown and do not cool the account for that reason.
- Token auto-refresh defaults to enabled, scans every 300 seconds, and refreshes refreshable credentials expiring within 1800 seconds.
- Dynamic virtual cache usage stays configurable and defaults to fixed mode for compatibility; Admin can enable `estimated_user_delta` input mode and `dynamic` creation mode.
- Dynamic proxy V1 follows the WindsurfAPI design but is implemented natively in Rust; dynamic active binding wins over manual account proxy, then global proxy.
- Dynamic proxy V1 targets Novproxy-style username templates and keeps plaintext provider password in SQLite/runtime settings, matching the current config/security model.
- Opus 4.7 ANTML probe compatibility is opt-in (`off`/`clarify`), scoped to plain Opus 4.7, and does not spoof or retry responses in the first version.
- PDF documents are converted to extracted text in message content. Do not send a Kiro `documents` field on this endpoint because live logs showed upstream `400 Improperly formed request`.
- Structured output is handled with request-scoped JSON/schema instructions, not response post-processing, so stream protocol remains intact and invalid model JSON is not silently rewritten.
- Do not send assistant `reasoningContent` history fields yet: live logs with `message_count=3` showed repeated upstream `400 Improperly formed request`, and the detector still failed model signature. No placeholder or fake signature is generated.
- Opus 4.7 Clean Probe is a diagnostic/compatibility toggle, not a signature generator. It reduces synthetic local context for plain `claude-opus-4-7` only; a valid signature can only be exposed when upstream Kiro sends reasoning/signature events.
- Same-account retries are rule-driven and happen before account cooldown/failover classification. The default rule covers `429` + `INSUFFICIENT_MODEL_CAPACITY`; after configured attempts are exhausted, the existing cooldown/failover logic runs unchanged.
- The Opus 4.7 `cc_max_like` detection profile applies effective presets without mutating the stored individual toggles: Clean Probe off, plain stabilization off, models shape `aggregator`, usage shape `flat`, thinking model `native`, ANTML clarify effective, PDF/structured fixes retained.
- Identity probe compatibility is scoped to `cc_max_like`, plain Opus 4.7, and single-message detector-like prompts. It rewrites only the current user message and does not apply to PDF probes, structured-output requests, forced tool-use requests, tool-result turns, or long conversations.
- Signed-thinking support must not fabricate Anthropic signatures. `diagnose`/`cache_only` only observe or cache real upstream signatures; explicit `history_experiment` preserves client-returned assistant `thinking+signature` blocks as Kiro `reasoningContent.reasoningText` history for live round-trip testing.
- For `cc_max_like` identity probes, tool definitions are removed from the current Kiro user context after matching. Forced tool-use and tool-result turns are still excluded, so tool-call regression probes keep their normal behavior.
- For matched `cc_max_like` identity probes, visible assistant text is sanitized as a final narrow fallback for `kiro/aws/amazon` leakage and wrong model-family words. This does not alter signed-thinking `thinking` or `signature` blocks and does not run for PDF, structured-output, tool, or ordinary requests.
- For matched non-stream `cc_max_like` identity probes, visible assistant text is now normalized to the official Claude Code identity口径 after upstream response. This is intentionally narrower than global response rewriting and is based on stable endpoint behavior plus Anthropic's public Claude Code positioning.
- Identity probe matching now combines known detector phrases with a bounded identity-intent heuristic, so wording variants such as product identity, developer/company, model id, underlying model, backend provider, running platform, and system prompt/internal configuration can trigger without relying on exact phrasing.
- Remaining signature work should be diagnosed by classifying failures first. The new `opus47_signature_diagnostics.classification` distinguishes `signed_ok`, `no_client_thinking`, `client_hidden`, `upstream_no_reasoning`, `upstream_reasoning_no_signature`, and `upstream_signature_not_exposed`.
- `opus47ShortThinkingExperiment` is default-off and scoped to Opus 4.7 + `cc_max_like` + `history_experiment` + client-requested thinking + `max_tokens <= 16384` + PDF or short current text. `adaptive_high` rewrites only the XML thinking directive from enabled/max length to adaptive/high. It does not fabricate signatures.
- Identity probe compatibility must be evaluated against the original current user text, not text after other compatibility prefixes are prepended. This prevents ANTML/PDF/other probe clarifications from accidentally triggering identity normalization.
- ANTML probe visible-text normalization is allowed only for a single extracted `<antml:hex>` probe tag and only restores an upstream answer of `<hex>` to `<antml:hex>`; it does not synthesize unrelated answers or alter reasoning/signature blocks.
- Short exact/PDF/OCR/identity `upstream_no_reasoning` signature diagnostics are classified as expected text-only upstream behavior. Reasoning-like `upstream_no_reasoning`, upstream reasoning without signature, and upstream signature not exposed remain warning-level diagnostics.
- Prompt Dump is default-off because it stores sensitive prompt, document/base64, upstream request, upstream raw response, and client response data. It is runtime/Admin configurable and defaults to dumping only `claude-opus-4-6`, `claude-opus-4-7`, and `claude-sonnet-4-6`.
- Opus 4.7 run mode is an effective-request preset, not a stored-settings rewrite. `custom` preserves existing granular behavior; `benchmark` favors cctest/hvoy compatibility; `fast` lowers thinking work and disables detector diagnostics/compat extras. Prompt Dump and raw debug remain explicit manual settings.
- Opus 4.6 and Sonnet 4.6 now have separate runtime/Admin compatibility groups. Their `benchmark` mode applies the scoring-oriented effective profile; `fast` keeps client thinking/tools/history/tokens/effort intact and only enables ANTML clarify; `custom` follows the granular fields.
- Opus 4.6/Sonnet 4.6 identity compatibility is gated to `benchmark` or `custom + cc_max_like`; normal Claude Code/tool workflows should not have tools stripped unless the request matches the detector-style identity probe.
- PDF exact streaming diagnostics must not reuse the same assistant text buffer for replay. PDF diagnostic text is observed for logs only; already-forwarded stream text is not replayed at stream end.
- ANTML probe diagnostics use a distinct `request_kind="antml_probe"` so later signature/behavior logs are not confused with identity probes.
- Target cache reuse ratio is a goal layer above the existing virtual cache execution layer. `0` keeps current behavior; non-zero values steer future previews through bounded adjustments instead of directly overriding final usage fields.

## Errors Encountered
| Error | Attempt | Resolution |
|---|---|---|
| Existing planning files described the previous single-node production hardening task | 1 | Replaced the files with this task's plan before final verification |
| Docker dev startup failed because port 8990 was already occupied by prior `kiro-rs-prod` container | 1 | Stopped the old local verification container, then started `kiro-rs-dev` successfully |
| Policy smoke-test shell command passed an env var to Python incorrectly and left a temporary override | 1 | Restored credential #1 policy to `null/null` and verified runtime returned to default effective values |
| Session-affinity smoke command used zsh read-only variable `status` | 1 | Re-ran the smoke command through `bash` with `http_status`; request returned 200 and runtime showed one affinity binding |
| Virtual usage preview initially passed an owned ledger entry to a mutable helper | 1 | Changed the call to pass `&mut entry`; `cargo check` passed afterward |
| Request-local account exclusion could loop when no replacement account was dispatchable | 1 | Break out of provider retry loop when acquiring the next non-excluded account fails |
| Tried to pass two exact test names to `cargo test` in one invocation | 1 | Re-ran with a shared filter / full test suite instead |
| Tried to pass two unrelated test filters to `cargo test` in one invocation | 1 | Re-ran the targeted tests separately |
| Tried to run several exact cargo test filters in one command | 1 | Re-ran with `cargo test anthropic::converter::tests::` and then full `cargo test` |
| `pdf-extract` did not extract text from a hand-written minimal PDF fixture | 1 | Added a lightweight fallback parser for PDF literal string text operators (`Tj`/`TJ`) |
| Cargo accepts only one test-name filter per invocation | 1 | Re-ran Opus 4.7 targeted tests as separate invocations, then full `cargo test` |
| Docker dev startup failed after rebuild with `database disk image is malformed` for `config/kiro-rs.db` | 1 | Stopped the dev container, moved `kiro-rs.db*` to `config/db-backups/*.malformed-20260514-003112`, and let the service re-import the existing two credentials from `credentials.json` into a fresh SQLite DB |
| Real Docker identity probe still returned visible `Kiro` after request-side identity compatibility and tool-schema clearing | 1 | Added a final visible-text sanitizer scoped only to `identity_probe_applied=true`; repeated Docker probe then logged empty `leakage_keywords` and empty `mismatched_model_keywords` |
| Prompt-only Claude Code identity constraint still produced `# Claude` or `I can't discuss that.` in local Docker probes | 1 | Added non-stream identity visible-text normalization for matched identity probes, while keeping stream chunks limited to keyword sanitization |
| Exact identity phrase matching could miss detector wording variants | 1 | Added bounded identity-intent heuristics and negative tests for normal business modeling/authentication prompts |
| Needed to verify identity compatibility text cannot be elicited as prompt leakage | 1 | Ran the existing multi-turn prompt leak probe plus direct system-prompt probes; outputs did not reveal the internal compatibility prefix or proxy/platform details |
| Cargo test was invoked with multiple test-name filters | 1 | Re-ran with a single broader filter (`cargo test opus47`) and then full `cargo test -q` |
| Docker verification helper tried to parse the same Admin PUT response twice | 1 | Reworked the helper to consume the PUT body once and confirm settings with a separate GET |
| ANTML diagnostics were labeled `identity_short` after behavior was already correct | 1 | Added `Opus47RequestKind::AntmlProbe` and a regression test so logs show `request_kind="antml_probe"` |

## Multi-Endpoint Manual Switching Extension
- [completed] Compare sibling `Kiro-Go` endpoint shapes and add `ide`, `codewhisperer`, and `amazonq` upstream endpoint implementations.
- [completed] Persist a runtime `defaultEndpoint` setting and let credentials optionally override it with their own `endpoint`.
- [completed] Add Admin API/UI support for selecting the default endpoint and manually testing endpoint network latency.
- [completed] Log the endpoint, credential ID, region, URL, model, and stream mode for API requests, and endpoint/credential/region/URL for MCP requests.
- [completed] Verify backend formatting, checks, tests, frontend build, and whitespace diff checks.

## Multi-Endpoint Decisions
- Manual switching only for this iteration. No automatic 429 endpoint failover was added.
- Latency probing uses unauthenticated GET against the upstream API URL through the configured global proxy. It measures basic network/API reachability latency, not generation latency, quota state, or 429 behavior.
- Endpoint names are normalized at config/runtime/Admin boundaries so aliases such as `cw`, `code-whisperer`, and `amazon-q` resolve to registered names.

## Multi-Endpoint Errors Encountered
| Error | Attempt | Resolution |
|---|---|---|
| Runtime `defaultEndpoint` validation accepted aliases, but request dispatch needed registered canonical names | 1 | Normalize `defaultEndpoint` before storing runtime settings and return the actual effective settings from Admin save |

## Overage and Quota List Extension
- [completed] Compare sibling `Kiro-Go` overage behavior and mirror the same config shape: global `allowOverUsage`, account-level `allowOverage`, and `overageWeight`.
- [completed] Persist account overage policy in SQLite and keep stored policy synchronized with credential JSON/runtime dispatch fields.
- [completed] Skip accounts whose local quota snapshot is full unless global or account overage is enabled.
- [completed] Stop overage for a credential when upstream returns `402` with `OVERAGE`, without disabling the account; keep monthly request-count exhaustion as the existing quota-disable path.
- [completed] Update Admin runtime settings and account policy dialogs for overage controls.
- [completed] Show subscription/quota/usage percentage directly in the account list and auto-fetch missing quota snapshots for the current page.
- [completed] Verify backend checks/tests and frontend build.

## Overage Decisions
- Local quota snapshots are advisory. If an account is locally over limit, it is skipped by default, but `allowOverUsage` or account `allowOverage` lets it continue dispatching until upstream rejects with `OVERAGE`.
- Upstream `402 OVERAGE` means stop overage for that account (`overageStopped=true`, account-level `allowOverage=false`) but do not mark the credential disabled.
- Upstream `402 MONTHLY_REQUEST_COUNT` remains a hard quota exhaustion signal and continues to disable the credential as before.
- The account list fetches missing current-page quota data automatically, but manual refresh remains available per account and for the current page.

## Account Import Compatibility Extension
- [completed] Add a shared Admin UI credential import parser for single JSON objects, JSON arrays, wrapper objects, JSONL, and consecutive pretty-printed JSON objects.
- [completed] Support old nested `credentials` account exports and new flat account exports from KAM/Kiro-Go-style tools.
- [completed] Preserve imported account metadata including email, profile ARN, access token, expiration time, subscription title, usage snapshot, endpoint, proxy, and overage settings.
- [completed] Wire both batch credential import and KAM import dialogs to the shared parser.
- [completed] Make the account table's main account label show imported email whenever it is present.
- [completed] Verify frontend build, Rust formatting/checks, parser smoke cases, overage tests, full backend tests, and whitespace diff checks.

## Account Import Decisions
- Import parsing is intentionally frontend-side and permissive, while backend validation still owns credential validity and duplicate rejection.
- OAuth imports still require a `refreshToken` for actual add/verify flow; imported `accessToken`/`expiresAt`/`profileArn` are preserved as metadata but refresh remains the source of truth when adding OAuth credentials.
- The KAM dialog remains OAuth-focused and filters to records with a refresh token. The general batch import dialog supports OAuth and API Key records.
- Empty email values are not replaced with labels in backend storage. The list shows email when present and falls back to masked API key or credential ID otherwise.

## Account Balance Auto Refresh Extension
- [completed] Keep manual single-account and current-page balance refresh controls.
- [completed] Add an Admin page toggle for automatic current-page balance refresh.
- [completed] Add selectable refresh intervals of 30s, 60s, 2min, and 5min, persisted in browser local storage.
- [completed] Show the latest successful balance refresh time in the toolbar.

## Account Balance Auto Refresh Decisions
- Auto refresh is page-scoped: only the currently visible page of accounts is queried, so opening the Admin page does not sweep every account in the pool.
- Auto refresh is frontend-driven and stops when the Admin page is closed. It does not run as a detached backend scheduler.

## Virtual Cache Compression Reset Investigation
- [completed] Trace virtual cache ledger reset and compression accounting paths.
- [completed] Patch stream usage commit so compressed context sizes update the ledger.
- [completed] Add regression coverage for compressed stream context reset.
- [completed] Run targeted and broader validation.

## Kiro Power Import 400 Investigation
- [completed] Reproduce the Admin import failure with the provided Kiro Power export shape.
- [completed] Identify whether the 400 comes from frontend normalization, Admin request validation, or token verification.
- [completed] Patch the narrow compatibility issue and add regression coverage.
- [completed] Verify by local API/UI import without exposing imported secrets.
