# Task Plan: Admin Table and Runtime Policy Management

## Goal
Upgrade `kiro.rs` Admin from a credential-card view to a table-oriented account pool manager, persist credentials and runtime policy in SQLite for single-node production use, and make global/account scheduling policy hot-editable from the Admin API/UI.

Current extension: add virtual cache usage accounting so Anthropic-compatible responses expose cache read/write token fields for new-api/cctest-style billing and audit.

Current extension: split 429 handling into account-level limits vs model-capacity errors, add bounded request-local account failover, and expose model cooldown state for single-node production operation.

Current extension: add configurable background token auto-refresh so expiring Social/IdC credentials are refreshed before first request latency is hit.

Current extension: add optional natural dynamic virtual cache usage accounting so ordinary input tokens and cache creation tokens do not have to stay fixed.

Current extension: add dynamic per-account IP proxy binding so each account can keep an isolated, renewable proxy session with verification and auto-rotation.

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
