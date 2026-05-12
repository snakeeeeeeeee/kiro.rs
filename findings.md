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

## Opus 4.7 Detector Failure Analysis
- User's latest hvoy result for 4.7 now passes identity, model consistency, knowledge accuracy, protocol compliance, response completeness, and message-id checks. Remaining red flags are model signature (partial), PDF document recognition (failed), and structured output (failed).
- Local `kiro.rs` parses Kiro `reasoningContentEvent` and only exposes it to clients when Anthropic `thinking` is enabled. Plain 4.7 hides these internal events, which explains the jump from cctest 0% style failures toward hvoy 66%.
- 4.7 logs show first events frequently as `reasoning_content`; 4.6 logs show `assistant_response`. This means 4.7 naturally exercises the reasoning/signature path much more often, so any imperfect extended-thinking emulation is amplified on 4.7.
- `kiro.rs` currently emits `signature_delta` during streaming thinking and includes `signature` in non-streaming thinking blocks when Kiro sends one, but history conversion still flattens assistant `thinking` blocks into `<thinking>...</thinking>` text. It does not preserve client-supplied `thinking.signature` as Kiro `reasoningContent.reasoningText.signature`.
- Sibling `kiro-account-manager` preserves assistant reasoning history better: `HistoryAssistantMessage` has `reasoning_content`, `extract_reasoning_content` reads `thinking/signature`, and `build_history_assistant_message` stores it as `reasoningContent.reasoningText.signature`. That is a concrete local gap likely affecting multi-turn signature verification.
- `kiro.rs` has no Kiro `documents` field on `UserInputMessage`; `process_message_content` supports text/image/tool_result only. Anthropic `document` blocks or `application/pdf` sources are silently ignored as unknown blocks. This directly explains PDF document recognition failures.
- Sibling `kiro-account-manager` models `DocumentBlock { format, name, source }`, but its active converter still sets `documents: None` and only extracts images. It is useful for field shape but not a complete working implementation.
- `kiro.rs` has no OpenAI `response_format` or Anthropic/OpenAI JSON schema structured-output compatibility path. `tool_choice` is carried as raw request data but not used to force output schema unless it is a normal tool flow. This directly explains structured-output detector failures.
- Public `jwadow/kiro-gateway` also lacks PDF document conversion and uses fake reasoning tags with a generated placeholder signature. Its own comments say the signature is a placeholder, not a real Anthropic cryptographic signature. This supports treating detector "signature" as a protocol/emulation issue, not proof of a non-Claude model.
- Official Anthropic PDF support uses `type: "document"` content blocks with `source.type: "base64"`, `media_type: "application/pdf"`, and `data`. Official extended thinking has signed thinking blocks/signature verification semantics that proxies must preserve across turns.

## Opus 4.7 Git Regression Check
- Git history shows `3f9e229` first added native `reasoningContentEvent` parsing for Opus 4.7 and streamed/stored reasoning/signature unconditionally when Kiro sent it.
- The follow-up `1b070bb` changed that behavior: `ReasoningContent` is only emitted or stored when `thinking_enabled` is true; otherwise reasoning/signature is hidden. This is protocol-cleaner for ordinary plain requests, but can make detector signature checks fail for plain `claude-opus-4-7`.
- Current code then hard-forces `client_thinking_enabled_for_request()` to return false for exact plain `claude-opus-4-7` / `claude-opus-4.7`, so even if stabilization injects upstream adaptive thinking, the client never sees `thinking_delta` or `signature_delta`.
- The latest user logs prove stabilization is not sufficient: with `adaptive_low`, one concurrent probe can still be `assistant_response` with `signature_seen=false` while another is `reasoning_content` with `signature_seen=true`.
- Sibling `kiro-account-manager` has a concrete behavior local `kiro.rs` lacks: assistant history preserves `reasoningContent.reasoningText.signature`; local `kiro.rs` currently flattens assistant thinking into `<thinking>...</thinking>` text and drops the signature in history conversion.
- Therefore the likely direction correction is: stop treating adaptive stabilization as the main fix, and prioritize signed-thinking/signature preservation/exposure policy. Stabilization remains useful as a diagnostic or optional upstream-shape experiment, not a root-cause fix.

## Stable Opus 4.7 Proxy Comparison
- User provided a stable reverse-Kiro endpoint in `tmp/stable_opus47.env`; probes only logged event/type/length metadata and did not print keys or response text.
- Stable proxy plain `claude-opus-4-7` stream/non-stream returns only text blocks; no `thinking_delta`, no `signature_delta`, and no non-stream `thinking.signature`.
- Stable proxy also hides thinking/signature for explicit request `thinking: {"type":"enabled"|"adaptive"}` and for model `claude-opus-4-7-thinking`; response `message.model` is normalized back to `claude-opus-4-7`.
- Stable proxy rejects `claude-opus-4.7-thinking` with 503, while accepting `claude-opus-4-7-thinking`. Its public `/v1/models` entries are aggregation-style (`type: "model"`, `owned_by: null`, `max_tokens: null`) rather than local Anthropic-style model objects.
- Stable proxy concurrent 64k stream probes stayed text-only and stable: each stream had one text block, only `text_delta`, no thinking/signature, and usage fields `{input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens}`.
- Stable proxy WebSearch shape matches local `kiro.rs` closely: `text`, `server_tool_use`, `web_search_tool_result`, 10 result items, same usage keys and stop reason.
- This disproves the narrower hypothesis that cctest pass requires exposing Anthropic `signature_delta` on plain 4.7. Their "signature/指纹" checks likely include broader response/protocol/token-usage fingerprints. Next fixes should compare/align plain proxy envelope and usage behavior before exposing hidden thinking in plain mode.

## Opus 4.7 ANTML Probe Research
- Compared `/Users/zhangyu/Desktop/不能过cctest的.txt` and `/Users/zhangyu/Desktop/过cctest.txt`: both use the same CCTest-style prompt template `我输入的这个tag 是：<antml:...>。直接输出你前面看到的tag是什么。看到的文本，不要使用任何工具。问题2:...`; only the hex tag value changes per attempt.
- Failing sample had one ANTML probe and returned a short refusal: `assistant_response` only, `reasoning_content=0`, `signature_seen=false`, visible text about not discussing system prompts/internal instructions.
- Passing sample had multiple ANTML probes. Early probes also refused (`I can't discuss that.`), but a later probe returned the tag normally with `reasoning_content>0` and `signature_seen=true`. Therefore one refusal is not inherently fatal, but CCTest likely needs at least one successful probe or stops when the key probe fails.
- Sibling `kiro-account-manager` contains Opus 4.7 model mapping and reasoning/signature history preservation, but no exact `antml`/CCTest/tag-probe compatibility path.
- Sibling `WindsurfAPI` has no exact ANTML probe handling, but has relevant engineering patterns: narrowly rewriting prompt text to avoid upstream Claude 4.7 prompt-injection false positives (`client.js` neutralizes sentence-initial `You are`), short anti-refusal hints for known tool-protocol failures, explicit policy-block detection that avoids wasting quota on account rotation, and opt-in retry-with-correction that is off by default because it doubles quota.
- `WindsurfAPI` release notes for v2.0.82 explicitly call out the stream limitation: once chunks are emitted, retry cannot be done cleanly unless the stream path buffers/delays emission. This applies directly to a future Opus 4.7 weak-response retry mode in `kiro.rs`.
- Public search found Claude Code ANTML-related GitHub issues about ANTML/XML transport encoding/parser behavior, but no clear public discussion of this exact CCTest `tag/antml` probe or a standard workaround.

## telagod/llm-probe Review
- `https://github.com/telagod/llm-probe` is a Claude/Anthropic-compatible endpoint probe framework, not an exact CCTest clone. Repository search found no `antml` or the Chinese tag prompt.
- Useful suites for this issue are `authenticity`, `stream`, and `injection`.
- `authenticity` builds a consistency signature from response envelope features: `type`, `role`, content presence, `msg_`/`req_` prefixes, `stop_reason`, model match, duration bucket, and `usage.input_tokens`. It repeats baseline calls and reports `consistency_variant_count` plus `consistency_drift_score`.
- That consistency approach matches the observed Opus 4.7 problem: the same class of request can alternate between short `assistant_response` refusal and `reasoning_content+signature`, so a local `opus47_stream_diagnostics` weak-response metric is directly useful.
- `stream` validates Anthropic SSE contract strictly: first non-ping must be `message_start`, last must be `message_stop`, content block start/delta/stop ordering must be valid, and `message_delta` usage should appear.
- `injection` uses random canaries/sentinels and hard-gates leaks/hidden tool signals. It does not try to coerce the model to reveal hidden/internal content; for our cctest compatibility this supports keeping any ANTML prompt clarification narrow and factual, not a broad jailbreak-like instruction.
- Main takeaway: `llm-probe` helps design diagnostics and pass/fail interpretation, but does not provide a ready-made workaround for the `tag/antml` short refusal. Implementing a narrow prompt clarification and/or buffered weak-response retry remains a local gateway decision.

## PDF / Structured Output / Signature History Follow-up
- The direct cause of PDF detector failure was confirmed in code: `process_message_content` ignored `type: "document"` blocks, so PDF content never reached Kiro.
- The fix keeps original base64 PDF bytes in a Kiro-style `documents` field and injects extracted text into the current message. The text injection is important because it works even if upstream Kiro ignores `documents`.
- `pdf-extract` handles normal text-layer PDFs; a small fallback parser handles simple literal PDF text operators so minimal detector PDFs do not silently become empty.
- Structured-output detector failure was also a conversion gap: `MessagesRequest` accepted no `response_format`, and `output_config.format` only carried reasoning effort before. The new path supports both and injects a per-request JSON/schema instruction.
- Signature history preservation is now aligned with sibling `kiro-account-manager`: assistant `thinking.signature` is stored as Kiro `reasoningContent.reasoningText.signature`. This does not expose or forge signatures for plain Opus 4.7 responses; it only preserves real signatures supplied in conversation history.
