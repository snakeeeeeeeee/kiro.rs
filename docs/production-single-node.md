# Single-Node Production Runbook

This guide is for running `kiro.rs` as a single-node production service. It does not assume Redis, Postgres, or multi-instance state sharing.

## Files

Use these files on the production host:

- `docker-compose-prod.yml`: production compose file
- `config/config.json`: service configuration
- `config/credentials.json`: initial Kiro credentials / JSON backup
- `config/kiro-rs.db`: SQLite source of truth after first startup
- `config/kiro_stats.json`: local usage stats, generated at runtime
- `config/kiro_balance_cache.json`: Admin balance cache, generated at runtime

Keep `config/` backed up. Do not commit it.

## Recommended Config

Runtime scheduling fields in `config.json` are used as the first SQLite seed. After startup, change concurrency, RPM, queue, cooldown, and load balancing from Admin UI.

For one account, seed conservatively:

```json
{
  "globalMaxConcurrent": 3,
  "globalMaxConcurrentLimit": 512,
  "perAccountMaxConcurrent": 3,
  "queueMaxSize": 64,
  "queueTimeoutMs": 30000,
  "perAccountRpm": 0,
  "globalRpm": 0,
  "rateLimitCooldownMs": 60000,
  "transientCooldownMs": 10000,
  "virtualCacheUsageEnabled": true,
  "virtualCacheDefaultTtl": "5m",
  "virtualCacheUncachedInputTokens": 1,
  "virtualCacheInputMode": "fixed",
  "virtualCacheMinInputTokens": 8,
  "virtualCacheMaxInputTokens": 96,
  "virtualCacheWarmupTokens": 18000,
  "virtualCacheMinCreationTokens": 128,
  "virtualCacheMaxCreationTokens": 1200,
  "virtualCacheCreationMode": "fixed",
  "virtualCacheCreationJitterRatio": 0.25,
  "virtualCacheBurstEveryTurns": 7,
  "virtualCacheBurstMinTokens": 1500,
  "virtualCacheBurstMaxTokens": 3000,
  "virtualCacheFallbackScope": "none",
  "targetCacheReuseRatio": 0,
  "shutdownDrainTimeoutSecs": 60
}
```

For more accounts:

- `globalMaxConcurrent`: start with `account_count * perAccountMaxConcurrent`
- `globalMaxConcurrentLimit`: safety cap for `globalMaxConcurrent`; raise it for large account pools after checking OS file descriptor and upstream limits
- `perAccountMaxConcurrent`: start with `2` or `3`
- `queueMaxSize`: start with `64` or `128`
- `rateLimitCooldownMs`: keep `60000` unless upstream 429 is rare
- `targetCacheReuseRatio`: optional soft target for recent 5-minute virtual-cache reuse; keep `0` to disable

Avoid high values until you have real traffic data.

On first startup, `credentials.json` is imported into `kiro-rs.db` if the database has no accounts. After that, SQLite is the source of truth. Keep JSON export enabled as backup/interoperability, but do not expect editing `credentials.json` to override an existing database.

## Start

Start from a published image:

```bash
docker compose -f docker-compose-prod.yml up -d
```

Build locally from this source tree and start:

```bash
DOCKER_BUILDKIT=1 docker compose -f docker-compose-prod.yml -f docker-compose-prod.build.yml build --progress=plain
docker compose -f docker-compose-prod.yml -f docker-compose-prod.build.yml up -d --no-build
```

Or use the helper script:

```bash
scripts/prod-up.sh
```

Run smoke checks after startup:

```bash
RUN_SMOKE=1 API_KEY='your-api-key' ADMIN_API_KEY='your-admin-api-key' scripts/prod-up.sh
```

## Overseas Server Deployment

On an overseas production server, Docker Hub access should usually be stable, so deploy from source with the local-build override:

```bash
git clone <your-repo-url> kiro.rs
cd kiro.rs
mkdir -p config
```

Create or upload:

- `config/config.json`
- `config/credentials.json`

Start with local build:

```bash
DOCKER_BUILDKIT=1 docker compose -f docker-compose-prod.yml -f docker-compose-prod.build.yml build --progress=plain
docker compose -f docker-compose-prod.yml -f docker-compose-prod.build.yml up -d --no-build
```

Equivalent helper:

```bash
scripts/prod-up.sh
```

The default Docker port mapping is `0.0.0.0:18990->8990`, so the service is reachable from outside the host if your firewall allows it:

```bash
curl -fsS http://127.0.0.1:8990/healthz
curl -fsS http://127.0.0.1:8990/readyz
API_KEY='your-api-key' ADMIN_API_KEY='your-admin-api-key' scripts/prod-smoke.sh
```

If you want to restrict Docker to localhost and expose it only through Nginx/Caddy, override the bind address:

```bash
KIRO_RS_BIND=127.0.0.1 \
scripts/prod-up.sh
```

When using the default public bind, control access with the server firewall/security group. Do not leave Admin UI open to untrusted networks.

Check status:

```bash
docker compose -f docker-compose-prod.yml ps
curl -fsS http://127.0.0.1:8990/healthz
curl -fsS http://127.0.0.1:8990/readyz
```

Run the production smoke test:

```bash
API_KEY='your-api-key' ADMIN_API_KEY='your-admin-api-key' scripts/prod-smoke.sh
```

If Opus is not available on the current account:

```bash
SKIP_OPUS=1 API_KEY='your-api-key' ADMIN_API_KEY='your-admin-api-key' scripts/prod-smoke.sh
```

## Network Exposure

`docker-compose-prod.yml` binds to `0.0.0.0:18990` by default and forwards it to container port `8990`.

Use environment variables only when you need a different bind address or host port:

```bash
KIRO_RS_BIND=127.0.0.1 KIRO_RS_PORT=18990 docker compose -f docker-compose-prod.yml up -d
```

If you keep the default public bind, limit inbound traffic to trusted IPs with your firewall/security group. Admin UI should not be reachable from the whole public internet.

## Upgrade

1. Back up config:

```bash
mkdir -p backups
tar -czf "backups/kiro-config-$(date +%Y%m%d-%H%M%S).tgz" config/
```

2. Pull or build and replace the container:

```bash
docker compose -f docker-compose-prod.yml pull
docker compose -f docker-compose-prod.yml up -d
```

If deploying a locally built image:

```bash
scripts/prod-up.sh
```

3. Run smoke:

```bash
API_KEY='your-api-key' ADMIN_API_KEY='your-admin-api-key' scripts/prod-smoke.sh
```

## Rollback

If the new build is bad, use the previous git revision or previous image tag, then restart:

```bash
git checkout <previous-good-commit>
scripts/prod-up.sh
```

Restore credentials only if the file itself was damaged or an operator made a bad credential change:

```bash
tar -xzf backups/kiro-config-YYYYmmdd-HHMMSS.tgz
docker compose -f docker-compose-prod.yml restart
```

## Smoke And Load Tests

Smoke test:

```bash
BASE_URL=http://127.0.0.1:8990 \
API_KEY='your-api-key' \
ADMIN_API_KEY='your-admin-api-key' \
scripts/prod-smoke.sh
```

Light load test:

```bash
BASE_URL=http://127.0.0.1:8990 \
API_KEY='your-api-key' \
ADMIN_API_KEY='your-admin-api-key' \
CONCURRENCY=3 \
REQUESTS=12 \
scripts/load-test.sh
```

For one account, keep `CONCURRENCY` at or below `perAccountMaxConcurrent` for baseline tests. Increase it only when validating `429` behavior.

TTFT and total latency test:

```bash
BASE_URL=http://127.0.0.1:8990 \
API_KEY='your-api-key' \
python3 scripts/ttft-load-test.py \
  --model claude-opus-4-8 \
  --concurrency 8 \
  --requests 40 \
  --warmup-requests 4
```

The TTFT test streams by default and reports first-token latency plus total latency percentiles. It writes `summary.json`, `requests.csv`, and `failures.jsonl` under `tmp/ttft-load-*/`.

Account-pool RPM test:

```bash
BASE_URL=http://127.0.0.1:8990 \
API_KEY='your-api-key' \
ADMIN_API_KEY='your-admin-api-key' \
python3 scripts/pool-rpm-test.py \
  --model claude-opus-4-7 \
  --profile mixed-agent \
  --start-rpm 10 \
  --step-rpm 10 \
  --max-rpm 100 \
  --step-seconds 120 \
  --sessions 100 \
  --stream
```

The RPM test sends stable `metadata.user_id` values across many simulated sessions, samples `/api/admin/runtime`, and writes `summary.json`, `summary.csv`, `requests.csv`, `runtime.csv`, and `failures.jsonl` under `tmp/pool-rpm-*/`. Use the reported `recommended_safe_rpm` as the first production limit, then adjust with real traffic data.

## Operations

View logs:

```bash
docker compose -f docker-compose-prod.yml logs -f --tail=200
```

View runtime status:

```bash
curl -fsS -H 'x-api-key: your-admin-api-key' \
  http://127.0.0.1:8990/api/admin/runtime | jq
```

Important fields:

- `globalInFlight`: active requests held by the global limiter
- `queueDepth`: requests waiting for a global permit
- `dispatchAvailableCredentials`: credentials available for new dispatch
- `coolingDownCredentials`: credentials temporarily skipped due to upstream errors
- `credentials[].inFlight`: active requests for each account
- `sessionAffinityEnabled`: whether soft session-to-account affinity is enabled
- `sessionAffinityBindings`: in-memory session-to-account bindings used to improve upstream cache locality

## Cache And Session Affinity

The proxy does not cache `/v1/messages` responses. Response caching is usually wrong for model calls because every request may contain different context, tools, or stream state.

For better upstream prompt-cache locality, keep the client session stable:

- Send a stable `metadata.user_id` on every request in the same conversation.
- Supported formats are `user_xxx_account__session_<uuid>` or JSON like `{"session_id":"<uuid>"}`.
- The proxy extracts that UUID and keeps a runtime-only soft binding from session to Kiro account for 12 hours.
- When the bound account is full, cooling down, disabled, RPM-limited, or incompatible with the requested model, the binding is removed and dispatch falls back to another available account.
- Bindings are not persisted to SQLite and are cleared on process restart.
- You can disable this routing behavior with `sessionAffinityEnabled=false` in runtime settings. This only changes account dispatch; virtual usage/cache accounting still uses the stable usage session key.

For highest cache hit rate, route retries and follow-up turns from the same external conversation through the same `metadata.user_id`. Avoid generating a new session UUID for every request. If your client has worker queues, partition by conversation/session ID so concurrent turns from one conversation do not scatter across accounts.

## Dynamic IP Binding

Dynamic IP binding is optional. It keeps a verified proxy session per account so accounts do not share the same outbound IP unless you choose to.

Effective proxy priority is:

1. Active dynamic IP binding
2. Manual account proxy
3. Global proxy from `config.json`
4. Direct connection

Configure it in Admin UI → `运行策略`:

- `动态 IP 绑定`: enable the dynamic proxy worker and request-path lookup.
- `新账号自动绑定`: automatically bind active accounts that do not have a binding.
- `动态代理协议`: `http` or `socks5`.
- `动态代理 Host/端口/密码`: provider connection details.
- `用户名模板`: supports `{region}`, `{state}`, `{sid}`, and `{ttl}`.
- `动态代理 TTL 分钟`: provider-side session lifetime.
- `动态代理提前续绑 ms`: worker rotates bindings before expiry.
- `出口验证 URL`: default `https://ipinfo.io/json`.

Novproxy-style example:

```text
Host: us.novproxy.io
Port: 1000
Username template: nfgr68136-region-{region}-st-{state}-sid-{sid}-t-{ttl}
Region: US
State: New Jersey
TTL minutes: 120
```

After saving settings, use the account table:

- `绑 IP`: create and verify a dynamic proxy binding for the account.
- `换 IP`: generate a new session and verify it.
- `验 IP`: verify the current binding and update the displayed egress IP.
- `清 IP`: remove the binding and fall back to manual/global proxy.
- Batch buttons are available after selecting multiple accounts.

The background worker rotates failed, expired, and soon-expiring bindings. If a request fails with a proxy/tunnel/auth/timeout-style error, the binding is marked failed and a rotate is scheduled. This avoids treating a bad proxy as a bad Kiro account.

Dynamic IP binding can help account/IP isolation and proxy reliability. It does not guarantee that model-capacity errors such as `INSUFFICIENT_MODEL_CAPACITY` disappear, because those are often upstream capacity or regional capacity signals.

## Virtual Cache Usage

The proxy can return Anthropic-compatible cache usage fields for downstream gateways such as new-api:

- `input_tokens`
- `cache_read_input_tokens`
- `cache_creation_input_tokens`
- `cache_creation.ephemeral_5m_input_tokens`
- `cache_creation.ephemeral_1h_input_tokens`
- `output_tokens`

This is virtual accounting for your single-node pool. It is not a claim that Kiro upstream billed the exact same cache read/write tokens.

The ledger is in memory and keyed by model plus the client/session scope derived from `metadata.user_id`. Internal Kiro credential rotation does not reset the virtual cache ledger for the same client session. When no metadata is present, requests use an isolated fallback key instead of a shared model bucket to avoid cross-user accounting.

By default the proxy keeps the older conservative accounting shape: `virtualCacheInputMode: "fixed"` uses `virtualCacheUncachedInputTokens`, and `virtualCacheCreationMode: "fixed"` uses the configured minimum/maximum creation range. For more natural downstream audit numbers, enable these in Admin runtime settings:

- `virtualCacheInputMode: "estimated_user_delta"` reports ordinary `input_tokens` from the latest user message estimate, clamped by `virtualCacheMinInputTokens` and `virtualCacheMaxInputTokens`.
- `virtualCacheCreationMode: "dynamic"` varies later-turn `cache_creation_input_tokens` from context delta, latest output size, deterministic jitter, and optional periodic burst creation.
- `virtualCacheCreationJitterRatio` controls variation. `0.25` means roughly plus/minus 25% before final clamping.
- `virtualCacheBurstEveryTurns` controls occasional larger creation writes. Set it to `0` to disable bursts.

External clients can choose the write bucket by sending Anthropic cache control:

```json
{
  "type": "text",
  "text": "...",
  "cache_control": {
    "type": "ephemeral",
    "ttl": "1h"
  }
}
```

If `ttl` is omitted or set to `5m`, cache creation is reported in `ephemeral_5m_input_tokens`. If `ttl` is `1h`, it is reported in `ephemeral_1h_input_tokens`.

## Troubleshooting

`/healthz` fails:

- The process is not running or the port is wrong.
- Check `docker compose -f docker-compose-prod.yml ps`.
- Check logs.

`/readyz` returns `503`:

- No credential is enabled.
- Use Admin UI or `/api/admin/credentials` to inspect credential state.

Many `429` responses:

- If message is queue full or queue timeout, lower client concurrency or raise `queueMaxSize`.
- If message says no dispatchable account, add accounts or lower client concurrency.
- If accounts are cooling down, check upstream 429/5xx frequency.

`inFlight` does not return to `0`:

- Check whether clients are holding long streams open.
- Run `scripts/prod-smoke.sh` after traffic stops.
- If the value stays non-zero with no clients, restart the single-node process and capture logs.

Token refresh problems:

- Check account `refreshFailureCount`.
- `invalid_grant` means the refresh token is no longer valid; re-import that account.
- Back up `config/credentials.json` before bulk changes.
