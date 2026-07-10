# IBKR CPAPI read-path wire layer + paper gateway harness (2026-07-10)

The **first slice of the IBKR venue adapter**: a transport-agnostic, IBKR-internal
**wire layer** for the Client Portal Web API (CPAPI) **REST read** endpoints, plus a
**hand-rolled** containerized Client Portal Gateway that authenticates a **paper**
account so we can capture real response JSON as test fixtures.

- **Status:** design — awaiting review, then implementation plan.
- **Scope:** one issue, one PR (wire crate + gateway harness).
- **Parallelism:** genuinely independent of the in-flight `net-http` hardening — the
  `wire` module depends on **`serde`/`serde_json` only** (no `oath-model`, no
  `net-*-api`), so nothing in flight can block or invalidate it. This is *why* this
  slice was chosen as the parallel track.

## 1. Context & motivation

OATH needs venue adapters; IBKR is the first ([README](../../../README.md) lists
`oath-adapter-ibkr` as "coming soon"). Today **no `oath-adapter-ibkr` crate exists**.

The adapter's real foundations are still empty skeletons:

- [`oath-adapter-api`](../../../crates/adapter/api/src/lib.rs) is a 3-line skeleton —
  there is **no `Broker` trait, no `DataProvider` trait** anywhere in the tree.
- [`oath-model`](../../../crates/model/src/) has only `error`, `price`, `quantity`,
  `side` — **no `InstrumentId`, `Instrument`, `Order`, or `OrderId`**. ADR-0025 and
  ADR-0026 define these on paper; they are unbuilt.

So any adapter work that touches the **OATH side** of the translation boundary is
blocked on building those foundations first (the contract-first path, deliberately
set aside for now). The one slice that is simultaneously (1) IBKR-specific,
(2) parallel to `net-http`, and (3) free of unbuilt OATH types is the **venue-side
half of [ADR-0003](../../../docs/adr/0003-canonical-model-adapter-translation.md)'s
anti-corruption boundary**: model the CPAPI wire, and defer *all* OATH-domain
translation until the domain types exist.

**API-surface decision: target Client Portal API v1 (`cpapi`) now; Web API (beta) and
TWS deferred.** The adapter is **surface-pluggable** (see §3.1). We build against **Client
Portal API v1** — IBKR's *current, stable, GA* REST surface, reached through the local
Client Portal Gateway (`https://localhost:5000/v1/api`, session + `/tickle`) — which
reuses the existing `net-http` stack. Two other surfaces are deliberately deferred as
separate future modules:

- **IBKR Web API** — IBKR's newer unified surface (`https://api.ibkr.com/v1/api`,
  OAuth 2.0 `private_key_jwt`). It is explicitly *"in beta and subject to change."* For the
  read path it documents *the same backend resources / JSON shapes* as CP API v1 — the real
  differences are **auth + base URL** plus additive `/gw/api`, `/oauth`, `/oauth2`
  families. Because it is beta and, for our read path, differs only in auth/transport, we
  do **not** couple to it now (see §7.5).
- **TWS / IB Gateway** — a proprietary length-prefixed TCP socket protocol needing a
  raw-TCP transport OATH lacks; a genuinely different wire.

This slice covers the CP API v1 **REST read path only**; order writes and WS streaming are
deferred (see §2). *(Surface landscape verified against IBKR docs 2026-07-10, medium
confidence — the beta Web API reference pages were not directly reachable, so the
"same read-path wire" claim is documented, not independently verified.)*

## 2. Scope

**In:**

- `wire` DTOs + (de)serialization for the read endpoints, plus the CPAPI error
  envelope(s):
  - `GET /iserver/auth/status` — session auth status
  - `POST /tickle` — session keepalive (+ session sub-object)
  - `GET /iserver/accounts` and `GET /portfolio/accounts` — tradable / portfolio accounts
  - `GET /portfolio/{accountId}/positions/{pageId}` — positions
  - `POST /iserver/secdef/search` — contract search by symbol/company
  - `GET /iserver/secdef/info` — contract details (after search)
- An **endpoint descriptor** (HTTP method + path template with typed params) per endpoint.
- **Fixture-based unit tests** that run in `just ci`.
- A **hand-rolled** Client Portal Gateway container + a capture workflow + a **gated**
  live integration test.

**Out (deferred on purpose):**

- Order **write** path (place / the two-step reply-confirm / cancel / modify) — couples
  to the unbuilt order-safety contract (ADR-0022 / ADR-0026), so it would churn.
- **WS** streaming envelopes and market-data snapshot — need a `net-ws` backend that
  does not exist yet.
- **Any translation to OATH domain types** — blocked on unbuilt `InstrumentId` / `Order`.
- The `Broker` / `DataProvider` trait impls.
- **Credential automation** — login is a manual browser SSO step (see §3.2).

## 3. Architecture

Two components: a Rust `wire` module (the parallel code) and a container harness (the
fixture source).

### 3.1 `oath-adapter-ibkr` crate — `wire` module (deep module)

- **New crate** at `crates/adapter/ibkr`, package `oath-adapter-ibkr`; added to
  `[workspace] members` and `[workspace.dependencies]`.
- **Dependencies:** `serde` (derive), `serde_json`, `thiserror` (for the error type).
  **No `oath-model`, no `net-*-api`.** Dev-deps: `serde_json` for fixtures (+ optional
  `pretty_assertions`).
- **Narrow public surface** — one typed struct per response: `AuthStatus`,
  `TickleResponse`, `Account`, `Position`, `SecdefSearchEntry`, `SecdefInfo` — plus a
  `CpapiError` envelope and an `Endpoint` descriptor (method enum + path template with
  typed params: `account_id`, `page_id`, `conid`).
- **Represents CPAPI's JSON faithfully** with idiomatic Rust: renamed fields
  (`#[serde(rename)]`), optional/missing fields (`Option`), numbers-sent-as-strings kept
  as `String`, string-or-number *polymorphic* fields via a tolerant `WireNum` enum **only
  if a captured fixture proves the field is polymorphic**, and ambiguous error shapes via
  an untagged enum. The layer mirrors the wire **losslessly
  and interprets no values** — string→number parsing and the mapping onto OATH's model
  (the anti-corruption *translation*) are the deferred second half (§2).
- **No transport.** The module maps `serde_json` ⇄ typed values; the future adapter
  feeds it response bytes from `net-http-hyper`. This keeps it a pure, offline-testable
  deep module — the property that makes it parallelizable.
- **Surface-neutral crate, per-surface modules.** The package name `oath-adapter-ibkr`
  is deliberately surface-agnostic. The Client Portal API v1 wire lives under a top-level
  `cpapi` module. Two future siblings are anticipated: `webapi` (IBKR's beta OAuth-2.0
  surface) and `tws` (the binary socket protocol). `tws` shares nothing with `cpapi`
  (a different wire); `webapi` documents the *same* read-path JSON as `cpapi` but a
  different auth/transport — so *if* it is built, whether to share the read-path DTOs is a
  decision for that time (§7.5), not now.
- **Module layout:** `cpapi/mod.rs` (re-exports, `Endpoint`, `CpapiError`),
  `cpapi/auth.rs`, `cpapi/portfolio.rs`, `cpapi/secdef.rs`. (Split out `cpapi::wire`
  later only if per-surface translation code joins.) Scale files to size.

### 3.2 Paper gateway harness (`docker/cpapi/`)

- **Hand-rolled Dockerfile** on a minimal JRE base (e.g. `eclipse-temurin:21-jre`;
  floor is Java 8u192+): download `clientportal.gw.zip` from IBKR
  (`https://download2.interactivebrokers.com/portal/clientportal.gw.zip`), unzip, copy
  our `root/conf.yaml`, expose `5000`, entrypoint `bin/run.sh root/conf.yaml`.
- The Client Portal Gateway is a **pure Java web server** — **no Xvfb/VNC/IBC** (that
  machinery is only for the TWS *desktop* app). The container stays lightweight.
- **No credentials baked in.** Authentication is a **manual one-time browser login** at
  `https://localhost:5000` with paper creds. No secret handling in the harness.
- `docker-compose.yml` for one-command bring-up; a `README` with the login + capture steps.
- **Session:** CPAPI brokerage sessions time out after **5 min idle**; `/tickle` ~every
  60 s. Capture and the live test run within a session window after manual login. A
  standing keepalive loop is **out of scope** for this slice (noted for later).
- **Fixture capture:** a `just ibkr-capture` recipe (`curl -k` against
  `https://localhost:5000/v1/api/…`) writes raw JSON to
  `crates/adapter/ibkr/tests/fixtures/cpapi/*.json`, followed by a documented **sanitization**
  pass. Only sanitized fixtures are committed.

## 4. Testing strategy (TDD, fixture-driven)

- **Red → green per endpoint:** write a test that deserializes
  `tests/fixtures/cpapi/<endpoint>.json` into the target DTO and asserts key fields → fails
  (type absent) → define the DTO → green.
- Fixtures are **real, sanitized paper-gateway responses** (captured via §3.2), so the
  tests encode *actual* CPAPI behaviour, not doc guesses.
- Round-trip (serialize) only where we also send a body; the read path is mostly `GET`
  (plus trivial-body `POST`s), so most tests assert deserialize.
- **Live integration test:** marked **`#[ignore]`** (needs a live, authenticated gateway).
  A cargo feature would *not* work — `just test` runs `--all-features`, which would switch
  the feature on in CI; `nextest`/`cargo test` skip `#[ignore]` tests by default, so
  `#[ignore]` keeps it out of `just ci` regardless. It shells `curl -k` at the running
  gateway and deserializes the live response with our DTOs. Run explicitly with
  `--run-ignored` (nextest) or `-- --ignored` (cargo test). Documented how to run.
- **CI:** only fixture-based unit tests run in `just ci`, so the DoD stays green offline.
  Include `just doc` in per-task verification (broken intra-doc links pass check/lint/test).

## 5. Workspace / lint conformance

- Compiles under `[workspace.lints]`: no `unsafe` (forbidden), **no `unwrap`/`expect`/
  indexing** in non-test code (custom serde helpers return `Result` + `thiserror`),
  `missing_docs` satisfied, edition 2024 / MSRV 1.90.
- `cargo-deny` / `typos` / `cargo-machete` clean. New deps (`serde`, `serde_json`) go in
  `[workspace.dependencies]`.
- Update the [README](../../../README.md) crate table + dependency-graph note for
  `oath-adapter-ibkr` (drop / update the "coming soon" example line).

## 6. Deliverables / Definition of done

- `crates/adapter/ibkr/` with the `cpapi` wire module + fixture tests passing in `just ci`.
- `docker/cpapi/` hand-rolled gateway (Dockerfile, compose, `conf.yaml`, README) that
  brings up an authenticated paper session; `just ibkr-capture` recipe.
- Gated live test present, excluded from CI.
- README updated; `CHANGELOG.md` `[Unreleased]` entry added (kept per project convention).
- `just ci` green.

## 7. Decisions (settled 2026-07-10)

1. **ADR — yes (short).** Record the surface choice: the IBKR adapter targets **Client
   Portal API v1 (`cpapi`)** — the current, stable, GA surface — with **IBKR Web API**
   (beta, OAuth 2.0) and **TWS** (socket) as deferred future surfaces, *not excluded*.
   Frame it as "cpapi first," not "cpapi instead of." References ADR-0003. The wire DTOs
   themselves need no ADR.
2. **Single crate + per-surface modules.** One `oath-adapter-ibkr` crate; CP API v1 under
   a `cpapi` module, with `webapi` and `tws` as possible future siblings. Revisit a
   dedicated crate only if a real boundary need appears (YAGNI).
3. **Fixture scrub:** strip account ids / balances / names (PII); keep `conid`s (public
   reference data). Confirm the exact field set against real captured JSON at capture time.
4. **Numeric handling — faithful mirror, no parse in the wire.** Wire structs represent
   what arrives *losslessly*: a number sent as a JSON string stays `String` (parsing it to
   a number is *interpretation* — i.e. translation, deferred per §2). Consistent numbers
   use `i64` (ids/counts) or `serde_json::Number` (precision-sensitive bare numbers). Each
   field is modelled as exactly what its captured fixture shows. **String-or-number
   *polymorphic* fields are not pre-modelled** — only if a captured fixture actually shows
   the same field arriving as both a string and a number do we introduce a tolerant
   `enum WireNum { Str(String), Num(serde_json::Number) }` for that field (acceptance, not
   conversion). All string→domain-number conversion (money → fixed-point
   per ADR-0023) lives in the later translation layer. `#[serde(rename)]` and `Option<T>`
   remain in use — those are lossless *structural* choices, not value interpretation.
5. **Web API (beta) — deferred, not pre-coupled.** IBKR's unified Web API is *"in beta and
   subject to change"* and, for our read path, differs from CP API v1 only in **auth + base
   URL** (OAuth 2.0 at `api.ibkr.com` vs gateway session at `localhost:5000`) — the
   response JSON is documented as the *same* backend resources. We therefore do **not**
   pre-build a shared wire for it (YAGNI; do not couple to an unstable beta). When/if a
   `webapi` module is actually built, decide *then* — against the beta as it stands, and
   after directly verifying its reference (unreachable this pass) — whether to extract the
   shared read-path DTOs or duplicate. Naming note: IBKR titles even the CP API v1 docs
   "Web API v1.0," so the future module's name should make the **auth/transport**
   distinction explicit (that, not the payload schema, is the real seam).
