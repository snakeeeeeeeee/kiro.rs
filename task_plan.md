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
- Signed-thinking support must not fabricate Anthropic signatures. Current implementation only observes/caches real upstream signatures in `diagnose`/`cache_only`; `history_experiment` remains an explicit gated entry for future shape tests.

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
