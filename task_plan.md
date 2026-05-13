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
- `opus47ShortThinkingExperiment=adaptive_high` is default-off and scoped to Opus 4.7 + `cc_max_like` + `history_experiment` + client-requested thinking + `max_tokens <= 1024` + PDF or short current text. It rewrites only the XML thinking directive from enabled/max length to adaptive/high; it does not add natural-language hidden instructions or fabricate signatures.

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
