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
