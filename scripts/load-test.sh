#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8990}"
API_KEY="${API_KEY:-${KIRO_RS_API_KEY:-z123456789}}"
ADMIN_API_KEY="${ADMIN_API_KEY:-${KIRO_RS_ADMIN_API_KEY:-$API_KEY}}"
MODEL="${MODEL:-claude-sonnet-4-5-20250929}"
CONCURRENCY="${CONCURRENCY:-3}"
REQUESTS="${REQUESTS:-12}"
TIMEOUT_SECS="${TIMEOUT_SECS:-120}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

if ! command -v curl >/dev/null 2>&1; then
  echo "missing required command: curl" >&2
  exit 2
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "missing required command: jq" >&2
  exit 2
fi

post_one() {
  local idx="$1"
  local payload="$TMP_DIR/payload-$idx.json"
  local body="$TMP_DIR/body-$idx.json"
  local meta="$TMP_DIR/meta-$idx.txt"

  jq -n \
    --arg model "$MODEL" \
    --arg prompt "Load test request $idx. Reply with exactly: ok $idx" \
    '{
      model: $model,
      max_tokens: 64,
      stream: false,
      messages: [{role: "user", content: $prompt}]
    }' > "$payload"

  curl -sS --max-time "$TIMEOUT_SECS" \
    -o "$body" \
    -w "$idx %{http_code} %{time_total}\n" \
    -X POST "$BASE_URL/v1/messages" \
    -H "content-type: application/json" \
    -H "x-api-key: $API_KEY" \
    --data-binary "@$payload" > "$meta" || {
      echo "$idx 000 0" > "$meta"
    }
}

echo "Load test target: $BASE_URL"
echo "model=$MODEL requests=$REQUESTS concurrency=$CONCURRENCY"

started=0
while [ "$started" -lt "$REQUESTS" ]; do
  batch=0
  while [ "$batch" -lt "$CONCURRENCY" ] && [ "$started" -lt "$REQUESTS" ]; do
    started=$((started + 1))
    batch=$((batch + 1))
    post_one "$started" &
  done
  wait
done

summary_file="$TMP_DIR/summary.tsv"
cat "$TMP_DIR"/meta-*.txt | sort -n > "$summary_file"

total="$(wc -l < "$summary_file" | tr -d ' ')"
ok="$(awk '$2 == 200 {count++} END {print count + 0}' "$summary_file")"
rate_limited="$(awk '$2 == 429 {count++} END {print count + 0}' "$summary_file")"
server_errors="$(awk '$2 >= 500 {count++} END {print count + 0}' "$summary_file")"
other="$((total - ok - rate_limited - server_errors))"
avg="$(awk '{sum += $3} END {if (NR == 0) print "0.000"; else printf "%.3f", sum / NR}' "$summary_file")"
p95="$(awk '{print $3}' "$summary_file" | sort -n | awk '{values[NR]=$1} END {if (NR == 0) print "0.000"; else {idx=int(NR*0.95); if (idx < 1) idx=1; printf "%.3f", values[idx]}}')"
p99="$(awk '{print $3}' "$summary_file" | sort -n | awk '{values[NR]=$1} END {if (NR == 0) print "0.000"; else {idx=int(NR*0.99); if (idx < 1) idx=1; printf "%.3f", values[idx]}}')"

echo
echo "Results"
echo "total=$total 200=$ok 429=$rate_limited 5xx=$server_errors other=$other"
echo "avg=${avg}s p95=${p95}s p99=${p99}s"

runtime_file="$TMP_DIR/runtime.json"
runtime_status="$(curl -sS --max-time "$TIMEOUT_SECS" -o "$runtime_file" -w '%{http_code}' \
  -H "x-api-key: $ADMIN_API_KEY" \
  "$BASE_URL/api/admin/runtime")"

if [ "$runtime_status" = "200" ]; then
  global_in_flight="$(jq -r '.globalInFlight' "$runtime_file")"
  queue_depth="$(jq -r '.queueDepth' "$runtime_file")"
  account_in_flight="$(jq -r '[.credentials[].inFlight] | add // 0' "$runtime_file")"
  cooling="$(jq -r '.coolingDownCredentials' "$runtime_file")"
  echo "runtime: globalInFlight=$global_in_flight queueDepth=$queue_depth accountInFlight=$account_in_flight cooling=$cooling"
else
  echo "runtime fetch failed: HTTP $runtime_status"
fi

if [ "${SHOW_ERRORS:-0}" = "1" ]; then
  echo
  echo "Non-200 bodies"
  awk '$2 != 200 {print $1, $2}' "$summary_file" | while read -r idx status; do
    echo "request=$idx status=$status"
    jq -c '.error // .' "$TMP_DIR/body-$idx.json" || cat "$TMP_DIR/body-$idx.json"
  done
fi
