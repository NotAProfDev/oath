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
fetch() {
  # $1 = method, $2 = path (relative to BASE), $3 = output filename
  curl -fksS -X "$1" "$BASE$2" -o "$OUT/$3"
  echo "captured $3"
}

fetch GET  /iserver/auth/status auth_status.json
fetch POST /tickle              tickle.json
fetch GET  /iserver/accounts    iserver_accounts.json
fetch GET  /portfolio/accounts  portfolio_accounts.json

if [ -n "$ACCOUNT" ]; then
  fetch GET "/portfolio/$ACCOUNT/positions/0" positions.json
else
  echo "skipping positions.json: pass an account id (arg 1 or IBKR_ACCOUNT)"
fi

curl -fksS -X POST "$BASE/iserver/secdef/search" \
  -H 'Content-Type: application/json' \
  -d '{"symbol":"AAPL","name":false,"secType":"STK"}' \
  -o "$OUT/secdef_search.json"
echo "captured secdef_search.json"

fetch GET "/iserver/secdef/info?conid=265598&secType=STK" secdef_info.json

echo "DONE. SANITIZE before committing: scrub account ids, balances, and names."
