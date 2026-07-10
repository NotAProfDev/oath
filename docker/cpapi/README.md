# IBKR Client Portal Gateway (paper) — fixture harness

A hand-rolled container that runs IBKR's Client Portal API **v1** gateway so we can
log in with a **paper** account and capture read-path responses as test fixtures.
The gateway is a plain Java web server — no Xvfb/VNC/IBC.

## Prerequisites
- Docker + Docker Compose.
- An IBKR **paper** account with API access enabled. (2FA on the paper login makes
  the manual browser step harder; disable it on the paper user if possible.)

## Run + authenticate
```bash
docker compose -f docker/cpapi/docker-compose.yml up -d --build
# open https://localhost:5000 in a browser, accept the self-signed cert,
# and log in with your PAPER credentials. Leave the tab; the session lives here.
```
The brokerage session times out after ~5 min idle; `/tickle` keeps it alive.
The image bakes a dev [`conf.yaml`](conf.yaml) whose `ips.allow` covers loopback +
RFC-1918 private ranges, so a browser login forwarded through Docker is **not**
rejected 403. `docker ps` shows the container `healthy` once the gateway is serving
(a `HEALTHCHECK` polls `/iserver/auth/status`).

## Capture fixtures
```bash
just ibkr-capture DU0000000     # your paper account id
```
This writes raw JSON to `crates/adapter/ibkr/tests/fixtures/cpapi/`. The recipe
resolves the gateway's container IP (localhost:5000 is not routable from inside a
devcontainer). If the script aborts on an endpoint, the brokerage session likely
isn't authenticated — log in at https://localhost:5000 and re-run.

## Sanitize before committing (required)
The responses come from a real paper account. Before `git add`:
- replace account ids (e.g. `DU…`/`U…`) with a placeholder like `DU0000000`,
- zero out balances / P&L / quantities,
- remove account holder names.
Keep `conid`s (public reference data). `gitleaks` runs in CI — no secrets.
