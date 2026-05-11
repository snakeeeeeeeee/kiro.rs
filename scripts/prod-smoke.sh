#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:8990}"
API_KEY="${API_KEY:-${KIRO_RS_API_KEY:-z123456789}}"
ADMIN_API_KEY="${ADMIN_API_KEY:-${KIRO_RS_ADMIN_API_KEY:-$API_KEY}}"
MODEL="${MODEL:-claude-sonnet-4-5-20250929}"
OPUS_MODEL="${OPUS_MODEL:-claude-opus-4-7}"
TIMEOUT_SECS="${TIMEOUT_SECS:-60}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 2
  fi
}

curl_json() {
  local output="$1"
  shift
  local status
  status="$(curl -sS --max-time "$TIMEOUT_SECS" -o "$output" -w '%{http_code}' "$@")"
  echo "$status"
}

assert_status() {
  local name="$1"
  local expected="$2"
  local actual="$3"
  local body="$4"
  if [ "$actual" != "$expected" ]; then
    echo "FAIL $name: expected HTTP $expected, got $actual" >&2
    cat "$body" >&2 || true
    exit 1
  fi
  echo "PASS $name"
}

assert_runtime_idle() {
  local runtime_file="$TMP_DIR/runtime-idle.json"
  local status
  status="$(curl_json "$runtime_file" \
    -H "x-api-key: $ADMIN_API_KEY" \
    "$BASE_URL/api/admin/runtime")"
  assert_status "runtime idle fetch" "200" "$status" "$runtime_file"

  local global_in_flight
  local queue_depth
  local account_in_flight
  global_in_flight="$(jq -r '.globalInFlight' "$runtime_file")"
  queue_depth="$(jq -r '.queueDepth' "$runtime_file")"
  account_in_flight="$(jq -r '[.credentials[].inFlight] | add // 0' "$runtime_file")"

  if [ "$global_in_flight" != "0" ] || [ "$queue_depth" != "0" ] || [ "$account_in_flight" != "0" ]; then
    echo "FAIL runtime idle: globalInFlight=$global_in_flight queueDepth=$queue_depth accountInFlight=$account_in_flight" >&2
    cat "$runtime_file" >&2
    exit 1
  fi
  echo "PASS runtime idle"
}

post_message() {
  local model="$1"
  local prompt="$2"
  local output="$3"
  local payload="$TMP_DIR/payload.json"
  jq -n \
    --arg model "$model" \
    --arg prompt "$prompt" \
    '{
      model: $model,
      max_tokens: 64,
      stream: false,
      messages: [{role: "user", content: $prompt}]
    }' > "$payload"

  curl_json "$output" \
    -X POST "$BASE_URL/v1/messages" \
    -H "content-type: application/json" \
    -H "x-api-key: $API_KEY" \
    --data-binary "@$payload"
}

require_cmd curl
require_cmd jq

echo "Smoke target: $BASE_URL"

health_file="$TMP_DIR/healthz.json"
health_status="$(curl_json "$health_file" "$BASE_URL/healthz")"
assert_status "healthz" "200" "$health_status" "$health_file"

ready_file="$TMP_DIR/readyz.json"
ready_status="$(curl_json "$ready_file" "$BASE_URL/readyz")"
assert_status "readyz" "200" "$ready_status" "$ready_file"

runtime_file="$TMP_DIR/runtime.json"
runtime_status="$(curl_json "$runtime_file" -H "x-api-key: $ADMIN_API_KEY" "$BASE_URL/api/admin/runtime")"
assert_status "admin runtime" "200" "$runtime_status" "$runtime_file"

dispatch_available="$(jq -r '.dispatchAvailableCredentials' "$runtime_file")"
if [ "$dispatch_available" -lt 1 ]; then
  echo "FAIL admin runtime: no dispatchable credentials" >&2
  cat "$runtime_file" >&2
  exit 1
fi
echo "PASS dispatchable credentials: $dispatch_available"

sonnet_file="$TMP_DIR/sonnet.json"
sonnet_status="$(post_message "$MODEL" "Say a short friendly hello." "$sonnet_file")"
assert_status "message $MODEL" "200" "$sonnet_status" "$sonnet_file"
sonnet_text="$(jq -r '[.content[]? | select(.type=="text") | .text] | join("")' "$sonnet_file")"
if [ -z "$sonnet_text" ] || [ "$(jq -r '.type // empty' "$sonnet_file")" != "message" ]; then
  echo "FAIL message $MODEL: invalid response text/type" >&2
  cat "$sonnet_file" >&2
  exit 1
fi
echo "PASS message response"

if [ "${SKIP_OPUS:-0}" != "1" ]; then
  opus_file="$TMP_DIR/opus.json"
  opus_status="$(post_message "$OPUS_MODEL" "Say a short friendly hello." "$opus_file")"
  assert_status "message $OPUS_MODEL" "200" "$opus_status" "$opus_file"
  opus_text="$(jq -r '[.content[]? | select(.type=="text") | .text] | join("")' "$opus_file")"
  if [ -z "$opus_text" ] || [ "$(jq -r '.type // empty' "$opus_file")" != "message" ]; then
    echo "FAIL message $OPUS_MODEL: invalid response text/type" >&2
    cat "$opus_file" >&2
    exit 1
  fi
  echo "PASS opus message response"
fi

concurrency="${CONCURRENCY:-3}"
echo "Running concurrency smoke: $concurrency requests"
for i in $(seq 1 "$concurrency"); do
  (
    out="$TMP_DIR/concurrent-$i.json"
    status="$(post_message "$MODEL" "Reply with exactly: concurrent $i" "$out")"
    echo "$i $status" > "$TMP_DIR/concurrent-$i.status"
  ) &
done
wait

non_200=0
for i in $(seq 1 "$concurrency"); do
  status="$(awk '{print $2}' "$TMP_DIR/concurrent-$i.status")"
  if [ "$status" != "200" ]; then
    non_200=$((non_200 + 1))
  fi
done

if [ "$non_200" -ne 0 ]; then
  echo "FAIL concurrency smoke: $non_200 non-200 responses" >&2
  for i in $(seq 1 "$concurrency"); do
    printf '%s ' "$i" >&2
    cat "$TMP_DIR/concurrent-$i.status" >&2
    cat "$TMP_DIR/concurrent-$i.json" >&2 || true
  done
  exit 1
fi
echo "PASS concurrency smoke"

assert_runtime_idle

echo "Smoke passed."
