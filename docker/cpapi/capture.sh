#!/usr/bin/env bash
# Capture Client Portal API v1 read-path responses from a running, authenticated
# gateway into the crate fixture directory. Log in first at https://localhost:5000.
# Usage: docker/cpapi/capture.sh [ACCOUNT_ID]   (or set IBKR_ACCOUNT)
# Must be run from the repo root — OUT below is repo-root-relative. `just ibkr-capture`
# already does this.
set -euo pipefail

BASE="${IBKR_GATEWAY:-https://localhost:5000/v1/api}"
OUT="crates/adapter/ibkr/tests/fixtures/cpapi"
ACCOUNT="${1:-${IBKR_ACCOUNT:-}}"
mkdir -p "$OUT"

# -f: abort on HTTP 4xx/5xx so an unauthenticated/expired session can't be silently written as a fixture.
# -k: the local gateway ships a self-signed TLS cert, so skip verification (dev-only; use --cacert if IBKR ships a CA).
# --max-time: fail fast instead of hanging if the gateway stalls or is only partially reachable.
fetch() {
  # $1 = method, $2 = path (relative to BASE), $3 = output filename
  curl -fksS --max-time 30 -X "$1" "$BASE$2" -o "$OUT/$3"
  echo "captured $3"
}

fetch GET  /iserver/auth/status auth_status.json
# POST /tickle with no body returns 411 (Length Required); send an empty body so
# curl sets Content-Length: 0.
curl -fksS --max-time 30 -X POST --data '' "$BASE/tickle" -o "$OUT/tickle.json"
echo "captured tickle.json"
fetch GET  /iserver/accounts    iserver_accounts.json
fetch GET  /portfolio/accounts  portfolio_accounts.json

if [ -n "$ACCOUNT" ]; then
  fetch GET "/portfolio/$ACCOUNT/positions/0" positions.json
else
  echo "skipping positions.json: pass an account id (arg 1 or IBKR_ACCOUNT)"
fi

curl -fksS --max-time 30 -X POST "$BASE/iserver/secdef/search" \
  -H 'Content-Type: application/json' \
  -d '{"symbol":"AAPL","name":false,"secType":"STK"}' \
  -o "$OUT/secdef_search.json"
echo "captured secdef_search.json"

# secType=STK triggers 400 "month required" on a stock; pass conid alone.
fetch GET "/iserver/secdef/info?conid=265598" secdef_info.json

# ---- Order write-path capture (paper account only; has real side effects) ----
# Places a deliberately far-below-market resting LIMIT BUY so it will NOT fill, drives
# the reply-confirm dance, reads status + live orders, then cancels. Override the
# contract with IBKR_CONID (default AAPL 265598).
if [ -n "$ACCOUNT" ]; then
  CONID="${IBKR_CONID:-265598}"
  # jget FILE KEY -> prints top-level element [0][KEY] of a JSON array, else "".
  jget() {
    python3 -c 'import json,sys
d=json.load(open(sys.argv[1]))
print(d[0].get(sys.argv[2],"") if isinstance(d,list) and d else "")' "$1" "$2"
  }

  curl -fksS --max-time 30 -X POST "$BASE/iserver/account/$ACCOUNT/orders" \
    -H 'Content-Type: application/json' \
    -d "{\"orders\":[{\"conid\":$CONID,\"orderType\":\"LMT\",\"side\":\"BUY\",\"quantity\":1,\"tif\":\"DAY\",\"price\":1.00,\"outsideRTH\":false}]}" \
    -o "$OUT/order_place.json"
  echo "captured order_place.json"

  reply_id=$(jget "$OUT/order_place.json" id)
  if [ -n "$reply_id" ]; then
    cp "$OUT/order_place.json" "$OUT/order_place_questions.json"
    for _ in 1 2 3 4 5; do
      curl -fksS --max-time 30 -X POST "$BASE/iserver/reply/$reply_id" \
        -H 'Content-Type: application/json' -d '{"confirmed":true}' \
        -o "$OUT/order_reply_confirmed.json"
      echo "captured order_reply_confirmed.json (reply $reply_id)"
      reply_id=$(jget "$OUT/order_reply_confirmed.json" id)
      if [ -z "$reply_id" ]; then break; fi
    done
    confirm_file="$OUT/order_reply_confirmed.json"
  else
    echo "no reply question was raised; order_place.json IS the confirmation."
    echo "  -> author order_place_questions.json as a documented representative fixture."
    confirm_file="$OUT/order_place.json"
  fi

  order_id=$(jget "$confirm_file" order_id)
  if [ -n "$order_id" ]; then
    if curl -fksS --max-time 30 -X GET "$BASE/iserver/account/order/status/$order_id" -o "$OUT/order_status.json"; then
      echo "captured order_status.json"
    else
      echo "WARNING: order_status fetch failed; continuing to cancel"
    fi
    if curl -fksS --max-time 30 -X GET "$BASE/iserver/account/orders" -o "$OUT/live_orders.json"; then
      echo "captured live_orders.json"
    else
      echo "WARNING: live_orders fetch failed; continuing to cancel"
    fi
    if curl -fksS --max-time 30 -X DELETE "$BASE/iserver/account/$ACCOUNT/order/$order_id" -o "$OUT/order_cancel.json"; then
      echo "captured order_cancel.json (cancelled order $order_id)"
    else
      echo "WARNING: cancel request failed — verify no resting order remains for $order_id"
    fi
    rm -f "$OUT/order_place.json"
  else
    echo "WARNING: no order_id parsed; skipping status/live/cancel. Inspect order_place/reply output."
  fi
else
  echo "skipping order write-path capture: pass an account id (arg 1 or IBKR_ACCOUNT)"
fi

echo "DONE. SANITIZE before committing: scrub account ids, balances, and names."
