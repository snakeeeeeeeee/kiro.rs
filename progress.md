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
