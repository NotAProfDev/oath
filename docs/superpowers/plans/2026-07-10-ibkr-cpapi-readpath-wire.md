# IBKR CPAPI Read-Path Wire Layer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first slice of the IBKR venue adapter — a transport-agnostic Client Portal API v1 (`cpapi`) **read-path** wire layer (serde DTOs + endpoint descriptors) with fixture tests, plus a hand-rolled paper-account gateway container for capturing those fixtures.

**Architecture:** A new `oath-adapter-ibkr` crate whose `cpapi` module holds IBKR-internal serde DTOs that faithfully mirror CP API v1 read responses — no auth, no transport, no OATH-domain translation. Fixtures are captured from a hand-rolled Client Portal Gateway container (paper login) and drive TDD. Depends only on `serde`/`serde_json`/`thiserror`, so it is fully parallel to the in-flight `net-http` work and blocked by nothing.

**Tech Stack:** Rust (edition 2024, MSRV 1.90), `serde`/`serde_json` derive, `thiserror`; Docker (`eclipse-temurin` JRE) for the gateway; `just` recipes; `curl` for capture/live test.

**Spec:** [docs/superpowers/specs/2026-07-10-ibkr-cpapi-readpath-wire-design.md](../specs/2026-07-10-ibkr-cpapi-readpath-wire-design.md). Grounds: ADR-0003 (adapter-side translation), ADR-0025/0026 (deferred domain types), ADR-0023 (fixed-point money, deferred).

## Global Constraints

*Every task's requirements implicitly include this section.*

- **Edition 2024, MSRV 1.90.** Validate with `just msrv` (`cargo +1.90 check --workspace --all-targets --all-features`).
- **`#![forbid(unsafe_code)]`** at crate root (workspace sets `unsafe_code = "deny"`; every crate's `lib.rs` forbids it — follow the pattern).
- **`just lint` runs `cargo clippy … -- -D warnings`.** Clippy `all` is deny; `pedantic`/`nursery`/`cargo` are `warn` but `-D warnings` promotes them to **hard errors**. Therefore, for every `pub` item and every `pub` field: a `///` doc comment (`missing_docs`); every type derives `Debug` (`missing_debug_implementations`); constructors returning a value get `#[must_use]` (`must_use_candidate`); public fns returning `Result` get a `/// # Errors` section (`missing_errors_doc`). Do **not** derive `PartialEq` without `Eq` on an `Eq`-capable type (`derive_partial_eq_without_eq`).
- **`unwrap`/`expect`/`indexing` = warn in non-test code**, but **test code is exempt** (`.clippy.toml`: `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-indexing-slicing-in-tests`). Non-test code returns `Result` / uses `.get()`.
- **Dependencies:** only `serde` (workspace dep — already carries `features = ["derive"]`), `serde_json` (workspace), `thiserror` (workspace). **No** `oath-model`, **no** `net-*-api`. `cargo-machete` runs in CI, so every declared dep must be used.
- **`just test` runs `--all-features`** → a cargo feature cannot exclude a test from CI. Use **`#[ignore]`** for the live test.
- **`just doc` runs `RUSTDOCFLAGS="-D warnings" cargo doc --document-private-items`** → broken intra-doc links fail even on private items. Cross-reference DTOs with **backticked names, not `[]` links**, to stay order-independent. Run `just doc` in every task's verification.
- **`just ci`** = `fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck`. New jargon → `_typos.toml`; shell scripts → shellcheck-clean (`set -euo pipefail`, quoted vars); **no secrets/credentials in committed files** (gitleaks); committed fixtures are **sanitized** (no account ids / balances / names).
- **Faithful-mirror rule (spec §7.4):** model each field as the wire actually sends it. Number-sent-as-string → `String`; ids/counts → `i64`; precision-sensitive numbers (money/quantity) → `serde_json::Number`; **no** `WireNum` enum unless a captured fixture proves a field arrives as both string and number; **no** OATH-domain translation (string→number parsing is deferred).
- **Workflow:** one issue, one PR. Isolate in a `.claude/worktrees/<slug>` git worktree — never switch the primary checkout's branch. Add a `CHANGELOG.md` `[Unreleased]` entry. `just ci` must pass before the PR.

## File Structure

```
crates/adapter/ibkr/
  Cargo.toml                         # package manifest (serde/serde_json/thiserror)
  src/
    lib.rs                           # crate docs + `#![forbid(unsafe_code)]` + `pub mod cpapi;`
    cpapi/
      mod.rs                         # module docs + submodule decls + facade re-exports
      endpoint.rs                    # `Method`, `Endpoint` (method + path template)
      error.rs                       # `CpapiError` (error envelope), `WireError`, `decode<T>`
      auth.rs                        # `AuthStatus`, `ServerInfo`, `TickleResponse`, `TickleIServer`
      portfolio.rs                   # `IServerAccounts`, `PortfolioAccount`, `Position`
      secdef.rs                      # `SecdefSearchEntry`, `SecdefSection`, `SecdefInfo`
  tests/
    endpoint.rs                      # path-rendering tests
    error.rs                         # decode success/error/malformed tests
    auth.rs                          # auth/tickle fixture tests
    portfolio.rs                     # accounts/positions fixture tests
    secdef.rs                        # secdef search/info fixture tests
    live.rs                          # `#[ignore]` live-gateway test
    fixtures/cpapi/*.json            # captured (sanitized) response fixtures

docker/cpapi/
  Dockerfile                         # hand-rolled Client Portal Gateway (JRE + clientportal.gw)
  docker-compose.yml                 # one-command bring-up on :5000
  capture.sh                         # shellcheck-clean fixture capture
  README.md                         # login + capture + sanitize + security notes
```

Root `Cargo.toml`, `_typos.toml`, `README.md`, `CHANGELOG.md`, `Justfile` are modified.

---

### Task 1: Crate scaffold, workspace registration, jargon allowlist

**Files:**
- Create: `crates/adapter/ibkr/Cargo.toml`
- Create: `crates/adapter/ibkr/src/lib.rs`
- Create: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Modify: `Cargo.toml` (workspace `members` + `workspace.dependencies`)
- Modify: `_typos.toml` (IBKR jargon)

**Interfaces:**
- Produces: the `oath-adapter-ibkr` crate compiling as an empty `cpapi` module; later tasks add submodules under `src/cpapi/`.

- [ ] **Step 1: Create the crate manifest**

`crates/adapter/ibkr/Cargo.toml`:

```toml
[package]
name = "oath-adapter-ibkr"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
```

- [ ] **Step 2: Create the crate root**

`crates/adapter/ibkr/src/lib.rs`:

```rust
//! IBKR venue adapter.
//!
//! Surface-neutral by design: the Client Portal API v1 wire layer lives under
//! [`cpapi`]. Future `webapi` (beta OAuth 2.0) and `tws` (socket) surfaces will be
//! siblings. This crate is the venue-side half of the ADR-0003 anti-corruption
//! boundary — it faithfully mirrors IBKR's wire and performs no translation to
//! OATH domain types (deferred until those types exist).
#![forbid(unsafe_code)]

pub mod cpapi;
```

- [ ] **Step 3: Create the (empty) `cpapi` module**

`crates/adapter/ibkr/src/cpapi/mod.rs`:

```rust
//! Client Portal API v1 (`cpapi`) read-path wire layer: endpoint descriptors and
//! serde DTOs that mirror IBKR's JSON responses losslessly. No auth, no transport,
//! no OATH-domain translation.
```

- [ ] **Step 4: Register the crate in the workspace**

In root `Cargo.toml`, add to `[workspace] members` immediately after the `"crates/adapter/api",` line:

```toml
  "crates/adapter/ibkr",
```

And in `[workspace.dependencies]`, immediately after the `oath-adapter-api = { … }` line:

```toml
oath-adapter-ibkr = { path = "crates/adapter/ibkr", version = "0.1.0" }
```

- [ ] **Step 5: Add IBKR jargon to the typos allowlist**

In `_typos.toml`, under `[default.extend-words]`, append (keep the existing `oath`/`strat` entries):

```toml
# IBKR Client Portal API v1 jargon (see crates/adapter/ibkr).
cpapi = "cpapi"
iserver = "iserver"
secdef = "secdef"
conid = "conid"
ssodh = "ssodh"
acct = "acct"
mkt = "mkt"
hmds = "hmds"
```

- [ ] **Step 6: Verify it compiles, formats, and passes typos**

Run: `cargo check -p oath-adapter-ibkr --locked`
Expected: compiles clean (no warnings).

Run: `just fmt-toml && just typos && just doc`
Expected: all pass. (If `typos` flags another IBKR token, add it to `_typos.toml` and re-run.)

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/ibkr/Cargo.toml crates/adapter/ibkr/src Cargo.toml Cargo.lock _typos.toml
git commit -m "feat(ibkr): scaffold oath-adapter-ibkr crate + cpapi module"
```

---

### Task 2: `Endpoint` + `Method` descriptors

**Files:**
- Create: `crates/adapter/ibkr/src/cpapi/endpoint.rs`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Test: `crates/adapter/ibkr/tests/endpoint.rs`

**Interfaces:**
- Produces: `Method { Get, Post }`; `Endpoint { method: Method, path: String }`; constructors `Endpoint::auth_status()`, `tickle()`, `iserver_accounts()`, `portfolio_accounts()`, `positions(account_id: &str, page: u32)`, `secdef_search()`, `secdef_info()`. Paths are relative to the gateway base `…/v1/api`.

- [ ] **Step 1: Write the failing test**

`crates/adapter/ibkr/tests/endpoint.rs`:

```rust
//! Endpoint path-rendering tests.
use oath_adapter_ibkr::cpapi::{Endpoint, Method};

#[test]
fn positions_path_interpolates_account_and_page() {
    let ep = Endpoint::positions("U1234567", 0);
    assert_eq!(ep.method, Method::Get);
    assert_eq!(ep.path, "/portfolio/U1234567/positions/0");
}

#[test]
fn tickle_is_a_post_to_slash_tickle() {
    let ep = Endpoint::tickle();
    assert_eq!(ep.method, Method::Post);
    assert_eq!(ep.path, "/tickle");
}

#[test]
fn secdef_search_is_a_post() {
    assert_eq!(Endpoint::secdef_search().method, Method::Post);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oath-adapter-ibkr --test endpoint`
Expected: FAIL — `cannot find … Endpoint`/`Method` in `oath_adapter_ibkr::cpapi`.

- [ ] **Step 3: Implement `endpoint.rs`**

`crates/adapter/ibkr/src/cpapi/endpoint.rs`:

```rust
//! Endpoint descriptors for the Client Portal API v1 read path.
//!
//! An [`Endpoint`] is a pure value — an HTTP [`Method`] plus a path *relative to the
//! gateway base URL* (`https://localhost:5000/v1/api`). This layer carries no
//! transport; a future HTTP binding turns an `Endpoint` into a request.

/// HTTP method for a Client Portal API v1 [`Endpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// HTTP `GET`.
    Get,
    /// HTTP `POST`.
    Post,
}

/// A Client Portal API v1 endpoint: an HTTP [`Method`] and a path relative to the
/// `/v1/api` base (for example `/portfolio/accounts`).
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// The HTTP method.
    pub method: Method,
    /// The path, relative to the `/v1/api` base.
    pub path: String,
}

impl Endpoint {
    /// `GET /iserver/auth/status` — current authentication / brokerage-session status.
    #[must_use]
    pub fn auth_status() -> Self {
        Self { method: Method::Get, path: "/iserver/auth/status".to_owned() }
    }

    /// `POST /tickle` — session keepalive; also relays the `iserver` auth status.
    #[must_use]
    pub fn tickle() -> Self {
        Self { method: Method::Post, path: "/tickle".to_owned() }
    }

    /// `GET /iserver/accounts` — accounts the user can trade.
    #[must_use]
    pub fn iserver_accounts() -> Self {
        Self { method: Method::Get, path: "/iserver/accounts".to_owned() }
    }

    /// `GET /portfolio/accounts` — accounts for portfolio/position queries; must be
    /// called before other `/portfolio` endpoints.
    #[must_use]
    pub fn portfolio_accounts() -> Self {
        Self { method: Method::Get, path: "/portfolio/accounts".to_owned() }
    }

    /// `GET /portfolio/{account_id}/positions/{page}` — one page of positions.
    #[must_use]
    pub fn positions(account_id: &str, page: u32) -> Self {
        Self {
            method: Method::Get,
            path: format!("/portfolio/{account_id}/positions/{page}"),
        }
    }

    /// `POST /iserver/secdef/search` — contract search by symbol / company name.
    #[must_use]
    pub fn secdef_search() -> Self {
        Self { method: Method::Post, path: "/iserver/secdef/search".to_owned() }
    }

    /// `GET /iserver/secdef/info` — contract details (call after `secdef_search`).
    #[must_use]
    pub fn secdef_info() -> Self {
        Self { method: Method::Get, path: "/iserver/secdef/info".to_owned() }
    }
}
```

- [ ] **Step 4: Wire the module + re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, append below the `//!` header:

```rust

pub mod endpoint;

pub use endpoint::{Endpoint, Method};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p oath-adapter-ibkr --test endpoint`
Expected: PASS (3 tests).

- [ ] **Step 6: Lint + doc**

Run: `just lint && just doc`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/ibkr/src/cpapi tests
git commit -m "feat(ibkr): cpapi Endpoint + Method descriptors"
```

---

### Task 3: `CpapiError` envelope, `WireError`, `decode<T>`

**Files:**
- Create: `crates/adapter/ibkr/src/cpapi/error.rs`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Test: `crates/adapter/ibkr/tests/error.rs`

**Interfaces:**
- Produces: `CpapiError { error: String, status_code: Option<i64> }`; `enum WireError { Json(serde_json::Error) }`; `fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError>`. `decode` is the single entry point later tasks (and the future transport) use to turn a response body into a typed value.

- [ ] **Step 1: Write the failing test**

`crates/adapter/ibkr/tests/error.rs`:

```rust
//! Tests for the CP API v1 error envelope and the `decode` entry point.
use oath_adapter_ibkr::cpapi::{decode, CpapiError, WireError};

#[test]
fn error_envelope_decodes() {
    let bytes = br#"{"error":"no bridge","statusCode":401}"#;
    let err: CpapiError = decode(bytes).expect("error envelope should decode");
    assert_eq!(err.error, "no bridge");
    assert_eq!(err.status_code, Some(401));
}

#[test]
fn error_envelope_without_status_code_decodes() {
    let bytes = br#"{"error":"Please query /accounts first"}"#;
    let err: CpapiError = decode(bytes).expect("bare error should decode");
    assert_eq!(err.status_code, None);
}

#[test]
fn malformed_json_is_a_wire_error() {
    let bytes = b"not json";
    let result: Result<CpapiError, WireError> = decode(bytes);
    assert!(matches!(result, Err(WireError::Json(_))));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p oath-adapter-ibkr --test error`
Expected: FAIL — `CpapiError`/`WireError`/`decode` not found.

- [ ] **Step 3: Implement `error.rs`**

`crates/adapter/ibkr/src/cpapi/error.rs`:

```rust
//! The Client Portal API v1 error envelope, this crate's decode error type, and the
//! [`decode`] entry point for turning a response body into a typed value.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

/// The JSON error body IBKR returns for a failed Client Portal API v1 request,
/// for example `{"error":"no bridge","statusCode":401}`.
#[derive(Debug, Clone, Deserialize)]
pub struct CpapiError {
    /// Human-readable error message.
    pub error: String,
    /// HTTP-style status code, when present.
    #[serde(rename = "statusCode")]
    pub status_code: Option<i64>,
}

/// An error decoding a Client Portal API v1 response body.
#[derive(Debug, Error)]
pub enum WireError {
    /// The body was not valid JSON for the target type.
    #[error("malformed Client Portal API JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Deserialize a Client Portal API v1 response body into `T`.
///
/// The wire layer carries no transport, so the caller (a future HTTP binding)
/// decides — from the HTTP status — whether to `decode::<T>` a success body or
/// `decode::<CpapiError>` an error body.
///
/// # Errors
///
/// Returns [`WireError::Json`] if `bytes` is not valid JSON for `T`.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    Ok(serde_json::from_slice(bytes)?)
}
```

- [ ] **Step 4: Wire the module + re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, add `pub mod error;` (below `pub mod endpoint;`) and extend the re-exports:

```rust
pub use error::{decode, CpapiError, WireError};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p oath-adapter-ibkr --test error`
Expected: PASS (3 tests).

- [ ] **Step 6: Lint + doc**

Run: `just lint && just doc`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/adapter/ibkr/src/cpapi tests
git commit -m "feat(ibkr): cpapi error envelope + decode entry point"
```

---

### Task 4: Auth/session DTOs (`AuthStatus`, `TickleResponse`)

**Files:**
- Create: `crates/adapter/ibkr/src/cpapi/auth.rs`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/auth_status.json`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/tickle.json`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Test: `crates/adapter/ibkr/tests/auth.rs`

**Interfaces:**
- Consumes: `decode` (Task 3).
- Produces: `AuthStatus`, `ServerInfo`, `TickleResponse`, `TickleIServer`.

> These fixtures are **representative** (documented shapes); Task 9 replaces them with real sanitized paper-gateway captures and the DTOs are reconciled to the real fields then. Model fields as the wire sends them (spec §7.4).

- [ ] **Step 1: Create the fixtures**

`crates/adapter/ibkr/tests/fixtures/cpapi/auth_status.json`:

```json
{"authenticated":true,"competing":false,"connected":true,"message":"","MAC":"00:00:00:00:00:00","serverInfo":{"serverName":"JifN00000","serverVersion":"Build 10.25.0"},"fail":""}
```

`crates/adapter/ibkr/tests/fixtures/cpapi/tickle.json`:

```json
{"session":"0000000000000000","ssoExpires":600000,"collision":false,"userId":100000000,"hmds":{"error":"no bridge"},"iserver":{"authStatus":{"authenticated":true,"competing":false,"connected":true,"message":"","MAC":"00:00:00:00:00:00","serverInfo":{"serverName":"JifN00000","serverVersion":"Build 10.25.0"},"fail":""}}}
```

- [ ] **Step 2: Write the failing test**

`crates/adapter/ibkr/tests/auth.rs`:

```rust
//! Fixture tests for the auth/session DTOs.
use oath_adapter_ibkr::cpapi::{decode, AuthStatus, TickleResponse};

#[test]
fn auth_status_deserializes() {
    let status: AuthStatus =
        decode(include_bytes!("fixtures/cpapi/auth_status.json")).expect("auth_status decodes");
    assert!(status.authenticated);
    assert!(status.connected);
    assert!(!status.competing);
}

#[test]
fn tickle_relays_iserver_auth_status() {
    let tickle: TickleResponse =
        decode(include_bytes!("fixtures/cpapi/tickle.json")).expect("tickle decodes");
    assert!(!tickle.session.is_empty());
    let iserver = tickle.iserver.expect("tickle relays the iserver block");
    assert!(iserver.auth_status.authenticated);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p oath-adapter-ibkr --test auth`
Expected: FAIL — `AuthStatus`/`TickleResponse` not found.

- [ ] **Step 4: Implement `auth.rs`**

`crates/adapter/ibkr/src/cpapi/auth.rs`:

```rust
//! Session/auth read endpoints: `iserver/auth/status` and `tickle`.

use serde::Deserialize;

/// Server identity block embedded in an [`AuthStatus`].
#[derive(Debug, Clone, Deserialize)]
pub struct ServerInfo {
    /// Server name.
    #[serde(rename = "serverName")]
    pub server_name: Option<String>,
    /// Server version string.
    #[serde(rename = "serverVersion")]
    pub server_version: Option<String>,
}

/// Response of `GET|POST /iserver/auth/status` — the brokerage-session state.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthStatus {
    /// `true` once initial authentication passes.
    pub authenticated: bool,
    /// `true` when another session is competing for the same account.
    pub competing: bool,
    /// `true` when connected to the brokerage backend.
    pub connected: bool,
    /// Optional status message.
    #[serde(default)]
    pub message: String,
    /// Machine access code, when present.
    #[serde(rename = "MAC")]
    pub mac: Option<String>,
    /// Failure reason; empty when healthy.
    #[serde(default)]
    pub fail: String,
    /// Server identity, when present.
    #[serde(rename = "serverInfo")]
    pub server_info: Option<ServerInfo>,
}

/// The `iserver` block of a [`TickleResponse`], wrapping the auth status.
#[derive(Debug, Clone, Deserialize)]
pub struct TickleIServer {
    /// The embedded auth status.
    #[serde(rename = "authStatus")]
    pub auth_status: AuthStatus,
}

/// Response of `POST /tickle` — session keepalive; also relays the auth status.
#[derive(Debug, Clone, Deserialize)]
pub struct TickleResponse {
    /// Opaque session token.
    pub session: String,
    /// SSO expiry, in seconds, when present.
    #[serde(rename = "ssoExpires")]
    pub sso_expires: Option<i64>,
    /// `true` when a session collision occurred.
    #[serde(default)]
    pub collision: bool,
    /// Numeric user id, when present.
    #[serde(rename = "userId")]
    pub user_id: Option<i64>,
    /// The relayed `iserver` auth block, when present.
    pub iserver: Option<TickleIServer>,
}
```

- [ ] **Step 5: Wire the module + re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, add `pub mod auth;` and:

```rust
pub use auth::{AuthStatus, ServerInfo, TickleIServer, TickleResponse};
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p oath-adapter-ibkr --test auth`
Expected: PASS (2 tests).

- [ ] **Step 7: Lint + doc, then commit**

Run: `just lint && just doc`
Expected: clean.

```bash
git add crates/adapter/ibkr
git commit -m "feat(ibkr): cpapi auth/tickle DTOs + fixtures"
```

---

### Task 5: Account DTOs (`IServerAccounts`, `PortfolioAccount`)

**Files:**
- Create: `crates/adapter/ibkr/src/cpapi/portfolio.rs`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/iserver_accounts.json`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/portfolio_accounts.json`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Test: `crates/adapter/ibkr/tests/portfolio.rs`

**Interfaces:**
- Consumes: `decode` (Task 3).
- Produces: `IServerAccounts`, `PortfolioAccount`. (`Position` is added in Task 6, same file.)

- [ ] **Step 1: Create the fixtures**

`crates/adapter/ibkr/tests/fixtures/cpapi/iserver_accounts.json`:

```json
{"accounts":["DU0000000"],"selectedAccount":"DU0000000","isPaper":true}
```

`crates/adapter/ibkr/tests/fixtures/cpapi/portfolio_accounts.json`:

```json
[{"id":"DU0000000","accountId":"DU0000000","currency":"USD","type":"DEMO","displayName":"Paper"}]
```

- [ ] **Step 2: Write the failing test**

`crates/adapter/ibkr/tests/portfolio.rs`:

```rust
//! Fixture tests for the portfolio DTOs.
use oath_adapter_ibkr::cpapi::{decode, IServerAccounts, PortfolioAccount};

#[test]
fn iserver_accounts_deserializes() {
    let accts: IServerAccounts = decode(include_bytes!("fixtures/cpapi/iserver_accounts.json"))
        .expect("iserver accounts decodes");
    assert_eq!(accts.accounts, vec!["DU0000000".to_owned()]);
    assert_eq!(accts.selected_account.as_deref(), Some("DU0000000"));
}

#[test]
fn portfolio_accounts_deserializes() {
    let accts: Vec<PortfolioAccount> =
        decode(include_bytes!("fixtures/cpapi/portfolio_accounts.json"))
            .expect("portfolio accounts decodes");
    assert_eq!(accts.len(), 1);
    let first = accts.first().expect("one account");
    assert_eq!(first.id, "DU0000000");
    assert_eq!(first.account_type.as_deref(), Some("DEMO"));
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p oath-adapter-ibkr --test portfolio`
Expected: FAIL — `IServerAccounts`/`PortfolioAccount` not found.

- [ ] **Step 4: Implement `portfolio.rs` (accounts)**

`crates/adapter/ibkr/src/cpapi/portfolio.rs`:

```rust
//! Portfolio read endpoints: `iserver/accounts`, `portfolio/accounts`, and
//! `portfolio/{account}/positions/{page}`.

use serde::Deserialize;

/// Response of `GET /iserver/accounts` — accounts the user can trade.
#[derive(Debug, Clone, Deserialize)]
pub struct IServerAccounts {
    /// Tradable account ids.
    pub accounts: Vec<String>,
    /// The currently selected account, when present.
    #[serde(rename = "selectedAccount")]
    pub selected_account: Option<String>,
}

/// One element of `GET /portfolio/accounts` — an account for portfolio queries.
#[derive(Debug, Clone, Deserialize)]
pub struct PortfolioAccount {
    /// Account id (for example `"DU0000000"`).
    pub id: String,
    /// Account id (a duplicate field IBKR also returns), when present.
    #[serde(rename = "accountId")]
    pub account_id: Option<String>,
    /// Base currency, when present.
    pub currency: Option<String>,
    /// Account type — IBKR's `type` field (`"DEMO"` for paper), when present.
    #[serde(rename = "type")]
    pub account_type: Option<String>,
}
```

- [ ] **Step 5: Wire the module + re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, add `pub mod portfolio;` and:

```rust
pub use portfolio::{IServerAccounts, PortfolioAccount};
```

- [ ] **Step 6: Run test, lint, doc, commit**

Run: `cargo test -p oath-adapter-ibkr --test portfolio` → PASS (2 tests).
Run: `just lint && just doc` → clean.

```bash
git add crates/adapter/ibkr
git commit -m "feat(ibkr): cpapi account DTOs + fixtures"
```

---

### Task 6: `Position` DTO (illustrates the faithful-mirror numeric rule)

**Files:**
- Modify: `crates/adapter/ibkr/src/cpapi/portfolio.rs`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/positions.json`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Modify: `crates/adapter/ibkr/tests/portfolio.rs`

**Interfaces:**
- Produces: `Position` — `conid: i64`; monetary/quantity fields as `serde_json::Number` (precision preserved; no `f64`, no parse — spec §7.4).

- [ ] **Step 1: Create the fixture**

`crates/adapter/ibkr/tests/fixtures/cpapi/positions.json`:

```json
[{"acctId":"DU0000000","conid":265598,"contractDesc":"AAPL","position":100,"mktPrice":150.25,"mktValue":15025.0,"currency":"USD","assetClass":"STK"}]
```

- [ ] **Step 2: Add the failing test**

Append to `crates/adapter/ibkr/tests/portfolio.rs`:

```rust
#[test]
fn positions_deserialize_conid_as_int_and_money_as_number() {
    use oath_adapter_ibkr::cpapi::Position;
    let positions: Vec<Position> =
        decode(include_bytes!("fixtures/cpapi/positions.json")).expect("positions decode");
    let p = positions.first().expect("one position");
    assert_eq!(p.conid, 265_598);
    // Money stays a serde_json::Number — faithful to the wire, no premature f64.
    assert_eq!(p.mkt_price.as_ref().map(ToString::to_string).as_deref(), Some("150.25"));
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p oath-adapter-ibkr --test portfolio positions_deserialize`
Expected: FAIL — `Position` not found.

- [ ] **Step 4: Implement `Position` in `portfolio.rs`**

Append to `crates/adapter/ibkr/src/cpapi/portfolio.rs`:

```rust
/// One element of `GET /portfolio/{account}/positions/{page}`.
///
/// `conid` is an **integer** on this endpoint (contrast `secdef/search`, where the
/// same logical id arrives as a *string* — see `SecdefSearchEntry`). Monetary and
/// quantity fields are kept as `serde_json::Number`: faithful to the wire, precision
/// preserved, no premature `f64`. Conversion to fixed-point (ADR-0023) is the future
/// translation layer's job, not the wire's.
#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    /// Account id owning the position, when present.
    #[serde(rename = "acctId")]
    pub acct_id: Option<String>,
    /// IBKR contract id (integer on this endpoint).
    pub conid: i64,
    /// Signed position size, when present.
    pub position: Option<serde_json::Number>,
    /// Market price, when present.
    #[serde(rename = "mktPrice")]
    pub mkt_price: Option<serde_json::Number>,
    /// Market value, when present.
    #[serde(rename = "mktValue")]
    pub mkt_value: Option<serde_json::Number>,
    /// Position currency, when present.
    pub currency: Option<String>,
    /// Contract description, when present.
    #[serde(rename = "contractDesc")]
    pub contract_desc: Option<String>,
}
```

- [ ] **Step 5: Extend the re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, extend the portfolio re-export to include `Position`:

```rust
pub use portfolio::{IServerAccounts, PortfolioAccount, Position};
```

- [ ] **Step 6: Run test, lint, doc, commit**

Run: `cargo test -p oath-adapter-ibkr --test portfolio` → PASS (3 tests).
Run: `just lint && just doc` → clean.

```bash
git add crates/adapter/ibkr
git commit -m "feat(ibkr): cpapi Position DTO (conid i64, money as Number)"
```

---

### Task 7: Secdef DTOs (`SecdefSearchEntry`, `SecdefInfo`)

**Files:**
- Create: `crates/adapter/ibkr/src/cpapi/secdef.rs`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/secdef_search.json`
- Create: `crates/adapter/ibkr/tests/fixtures/cpapi/secdef_info.json`
- Modify: `crates/adapter/ibkr/src/cpapi/mod.rs`
- Test: `crates/adapter/ibkr/tests/secdef.rs`

**Interfaces:**
- Produces: `SecdefSearchEntry` (`conid: String`!), `SecdefSection`, `SecdefInfo` (`conid: i64`). The string-vs-int `conid` split across these two endpoints is the concrete payoff of the faithful-mirror rule.

- [ ] **Step 1: Create the fixtures**

`crates/adapter/ibkr/tests/fixtures/cpapi/secdef_search.json`:

```json
[{"conid":"265598","companyName":"APPLE INC","symbol":"AAPL","description":"NASDAQ","sections":[{"secType":"STK"},{"secType":"OPT","months":"JAN26;FEB26"}]}]
```

`crates/adapter/ibkr/tests/fixtures/cpapi/secdef_info.json`:

```json
[{"conid":265598,"symbol":"AAPL","secType":"STK","exchange":"NASDAQ","currency":"USD","companyName":"APPLE INC"}]
```

- [ ] **Step 2: Write the failing test**

`crates/adapter/ibkr/tests/secdef.rs`:

```rust
//! Fixture tests for the secdef DTOs. Note the deliberate conid type split:
//! secdef/search sends conid as a string; secdef/info sends it as an integer.
use oath_adapter_ibkr::cpapi::{decode, SecdefInfo, SecdefSearchEntry};

#[test]
fn secdef_search_conid_is_a_string() {
    let entries: Vec<SecdefSearchEntry> =
        decode(include_bytes!("fixtures/cpapi/secdef_search.json")).expect("search decodes");
    let e = entries.first().expect("one entry");
    assert_eq!(e.conid, "265598");
    assert_eq!(e.sections.len(), 2);
}

#[test]
fn secdef_info_conid_is_an_int() {
    let infos: Vec<SecdefInfo> =
        decode(include_bytes!("fixtures/cpapi/secdef_info.json")).expect("info decodes");
    let i = infos.first().expect("one info");
    assert_eq!(i.conid, 265_598);
    assert_eq!(i.sec_type.as_deref(), Some("STK"));
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p oath-adapter-ibkr --test secdef`
Expected: FAIL — `SecdefSearchEntry`/`SecdefInfo` not found.

- [ ] **Step 4: Implement `secdef.rs`**

`crates/adapter/ibkr/src/cpapi/secdef.rs`:

```rust
//! Contract search/info read endpoints: `iserver/secdef/search` and
//! `iserver/secdef/info`.

use serde::Deserialize;

/// A tradable section within a [`SecdefSearchEntry`] (for example `STK`, `OPT`).
#[derive(Debug, Clone, Deserialize)]
pub struct SecdefSection {
    /// Security type, for example `"STK"`, `"OPT"`.
    #[serde(rename = "secType")]
    pub sec_type: String,
    /// Available expiry months (`OPT`/`FUT`), when present.
    pub months: Option<String>,
}

/// One element of `POST /iserver/secdef/search`.
///
/// `conid` is a **string** on this endpoint — the same logical id is an integer on
/// the positions and `secdef/info` endpoints. Modelling each as the wire actually
/// sends it (not a forced shared type) is the faithful-mirror rule (spec §7.4).
#[derive(Debug, Clone, Deserialize)]
pub struct SecdefSearchEntry {
    /// IBKR contract id (a string on this endpoint).
    pub conid: String,
    /// Company name, when present.
    #[serde(rename = "companyName")]
    pub company_name: Option<String>,
    /// Symbol, when present.
    pub symbol: Option<String>,
    /// Free-text description (often the exchange), when present.
    pub description: Option<String>,
    /// Tradable sections by security type.
    #[serde(default)]
    pub sections: Vec<SecdefSection>,
}

/// One element of `GET /iserver/secdef/info`.
///
/// `conid` is an **integer** here (contrast [`SecdefSearchEntry`]).
#[derive(Debug, Clone, Deserialize)]
pub struct SecdefInfo {
    /// IBKR contract id (an integer on this endpoint).
    pub conid: i64,
    /// Symbol, when present.
    pub symbol: Option<String>,
    /// Security type, when present.
    #[serde(rename = "secType")]
    pub sec_type: Option<String>,
    /// Primary exchange, when present.
    pub exchange: Option<String>,
    /// Contract currency, when present.
    pub currency: Option<String>,
    /// Company name, when present.
    #[serde(rename = "companyName")]
    pub company_name: Option<String>,
}
```

- [ ] **Step 5: Wire the module + re-exports**

In `crates/adapter/ibkr/src/cpapi/mod.rs`, add `pub mod secdef;` and:

```rust
pub use secdef::{SecdefInfo, SecdefSearchEntry, SecdefSection};
```

- [ ] **Step 6: Run test, lint, doc, commit**

Run: `cargo test -p oath-adapter-ibkr --test secdef` → PASS (2 tests).
Run: `just lint && just doc` → clean.

```bash
git add crates/adapter/ibkr
git commit -m "feat(ibkr): cpapi secdef DTOs (conid string vs int)"
```

---

### Task 8: Hand-rolled paper gateway harness

**Files:**
- Create: `docker/cpapi/Dockerfile`
- Create: `docker/cpapi/docker-compose.yml`
- Create: `docker/cpapi/capture.sh`
- Create: `docker/cpapi/README.md`
- Modify: `Justfile` (add `ibkr-capture` recipe)

**Interfaces:**
- Produces: a buildable Client Portal Gateway container, a `just ibkr-capture [account]` recipe, and documentation for logging in + capturing fixtures. This is empirical infra — its deliverable is verified by building the image and reaching the login page, not by a unit test.

- [ ] **Step 1: Write the Dockerfile**

`docker/cpapi/Dockerfile`:

```dockerfile
# Hand-rolled IBKR Client Portal Gateway (CP API v1). The gateway is a plain Java
# web server — no Xvfb/VNC/IBC (that machinery is only for the TWS desktop app).
FROM eclipse-temurin:21-jre

RUN apt-get update \
 && apt-get install -y --no-install-recommends unzip curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/clientportal

# Download + unpack the gateway distribution (extracts bin/, root/, dist/, ...).
ADD https://download2.interactivebrokers.com/portal/clientportal.gw.zip clientportal.gw.zip
RUN unzip -q clientportal.gw.zip && rm clientportal.gw.zip

EXPOSE 5000
# Uses the distribution's shipped root/conf.yaml (listens on :5000).
CMD ["bin/run.sh", "root/conf.yaml"]
```

- [ ] **Step 2: Write the compose file**

`docker/cpapi/docker-compose.yml`:

```yaml
services:
  cpgw:
    build: .
    image: oath-cpapi-gw
    container_name: oath-cpapi-gw
    ports:
      - "5000:5000"
    restart: unless-stopped
    # If the browser login is blocked (403 / "not allowed"), the gateway's shipped
    # root/conf.yaml ips.allow is too strict for container networking. Copy the
    # shipped root/conf.yaml out, widen ips.allow for local dev, and bind-mount it:
    # volumes:
    #   - ./conf.override.yaml:/opt/clientportal/root/conf.yaml:ro
```

- [ ] **Step 3: Write the capture script (shellcheck-clean)**

`docker/cpapi/capture.sh`:

```bash
#!/usr/bin/env bash
# Capture Client Portal API v1 read-path responses from a running, authenticated
# gateway into the crate fixture directory. Log in first at https://localhost:5000.
# Usage: docker/cpapi/capture.sh [ACCOUNT_ID]   (or set IBKR_ACCOUNT)
set -euo pipefail

BASE="${IBKR_GATEWAY:-https://localhost:5000/v1/api}"
OUT="crates/adapter/ibkr/tests/fixtures/cpapi"
ACCOUNT="${1:-${IBKR_ACCOUNT:-}}"
mkdir -p "$OUT"

fetch() {
  # $1 = method, $2 = path (relative to BASE), $3 = output filename
  curl -ksS -X "$1" "$BASE$2" -o "$OUT/$3"
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

curl -ksS -X POST "$BASE/iserver/secdef/search" \
  -H 'Content-Type: application/json' \
  -d '{"symbol":"AAPL","name":false,"secType":"STK"}' \
  -o "$OUT/secdef_search.json"
echo "captured secdef_search.json"

fetch GET "/iserver/secdef/info?conid=265598&secType=STK" secdef_info.json

echo "DONE. SANITIZE before committing: scrub account ids, balances, and names."
```

- [ ] **Step 4: Write the harness README**

`docker/cpapi/README.md`:

````markdown
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
If the login page rejects you (403 / "not allowed"), the shipped `root/conf.yaml`
`ips.allow` is too strict for container networking — see the commented bind-mount in
`docker-compose.yml`.

## Capture fixtures
```bash
just ibkr-capture DU0000000     # your paper account id
```
This writes raw JSON to `crates/adapter/ibkr/tests/fixtures/cpapi/`.

## Sanitize before committing (required)
The responses come from a real paper account. Before `git add`:
- replace account ids (e.g. `DU…`/`U…`) with a placeholder like `DU0000000`,
- zero out balances / P&L / quantities,
- remove account holder names.
Keep `conid`s (public reference data). `gitleaks` runs in CI — no secrets.
````

- [ ] **Step 5: Make the script executable + add the `just` recipe**

```bash
chmod +x docker/cpapi/capture.sh
```

In `Justfile`, add near the other recipes:

```make
# Capture Client Portal API v1 read-path fixtures from a running, authenticated
# gateway (see docker/cpapi/README.md). Pass a paper account id.
ibkr-capture account="":
    docker/cpapi/capture.sh {{account}}
```

- [ ] **Step 6: Verify the harness**

Run: `docker compose -f docker/cpapi/docker-compose.yml build`
Expected: image builds successfully.

Run: `shellcheck docker/cpapi/capture.sh`
Expected: no findings.

Run: `just --list | grep ibkr-capture`
Expected: the recipe is listed.

Manual (empirical): `docker compose -f docker/cpapi/docker-compose.yml up -d` then open `https://localhost:5000` — the login page loads. (If blocked, apply the `conf.override.yaml` bind-mount from Step 2.)

- [ ] **Step 7: Commit**

```bash
git add docker/cpapi Justfile
git commit -m "feat(ibkr): hand-rolled Client Portal Gateway harness + capture recipe"
```

---

### Task 9: Capture & commit real paper-account fixtures

**Files:**
- Modify: `crates/adapter/ibkr/tests/fixtures/cpapi/*.json` (replace representative with real, sanitized)
- Modify (as needed): `crates/adapter/ibkr/src/cpapi/{auth,portfolio,secdef}.rs` to reconcile with real fields

**Interfaces:**
- Consumes: the harness (Task 8) and the DTOs (Tasks 4–7).
- Produces: **real, sanitized** fixtures that the existing fixture tests pass against — satisfying the spec's "real captured fixtures" DoD.

> **Human-gated:** needs the paper account. Do this before merging; it replaces the representative fixtures from Tasks 4–7 with reality and reconciles any DTO field differences (the tests are your guide).

- [ ] **Step 1: Bring up the gateway and log in**

Run: `docker compose -f docker/cpapi/docker-compose.yml up -d --build`, then log in with paper credentials at `https://localhost:5000` (see `docker/cpapi/README.md`).

- [ ] **Step 2: Capture**

Run: `just ibkr-capture <YOUR_PAPER_ACCOUNT_ID>`
Expected: the seven `*.json` files under `tests/fixtures/cpapi/` are overwritten with live responses.

- [ ] **Step 3: Sanitize**

Edit each fixture: replace account ids with `DU0000000`, zero balances/P&L/quantities, remove names. Keep `conid`s.

Run (verify JSON is still valid): `for f in crates/adapter/ibkr/tests/fixtures/cpapi/*.json; do python3 -c "import json,sys; json.load(open(sys.argv[1]))" "$f"; done`
Expected: no output (all valid).

- [ ] **Step 4: Run the fixture tests against real data; reconcile**

Run: `cargo test -p oath-adapter-ibkr`
Expected: PASS. If a test fails because a real field differs from the representative shape, adjust the DTO in `src/cpapi/*.rs` (rename via `#[serde(rename)]`, make a field `Option`, or fix a type per the wire) and the assertion, honoring the faithful-mirror rule. Re-run until green.

- [ ] **Step 5: Guard against leaked account ids**

Run: `git grep -nE 'U[0-9]{7}' -- crates/adapter/ibkr/tests/fixtures || echo "clean"`
Expected: `clean` (only the `DU0000000` placeholder remains, which does not match).

- [ ] **Step 6: Lint, doc, commit**

Run: `just lint && just doc` → clean.

```bash
git add crates/adapter/ibkr
git commit -m "test(ibkr): real sanitized paper-gateway fixtures + DTO reconcile"
```

---

### Task 10: Gated live integration test

**Files:**
- Create: `crates/adapter/ibkr/tests/live.rs`

**Interfaces:**
- Consumes: `decode`, `AuthStatus`. Shells `curl -k` at the running gateway. `#[ignore]`d so it stays out of `just ci` (which runs `--all-features`, so a cargo feature would not exclude it).

- [ ] **Step 1: Write the ignored live test**

`crates/adapter/ibkr/tests/live.rs`:

```rust
//! Live integration test against a running, authenticated Client Portal Gateway.
//!
//! `#[ignore]` keeps it out of `just ci` — `just test` runs `--all-features`, so a
//! cargo feature would NOT exclude it, but nextest/cargo test skip ignored tests.
//! Run it explicitly (gateway up + logged in at https://localhost:5000):
//!   cargo test -p oath-adapter-ibkr --test live -- --ignored
//!   # or: cargo nextest run -p oath-adapter-ibkr --run-ignored
use std::process::Command;

use oath_adapter_ibkr::cpapi::{decode, AuthStatus};

#[test]
#[ignore = "requires a live, authenticated Client Portal Gateway on https://localhost:5000"]
fn live_auth_status_deserializes() {
    let base = std::env::var("IBKR_GATEWAY")
        .unwrap_or_else(|_| "https://localhost:5000/v1/api".to_owned());
    let output = Command::new("curl")
        .args(["-ksS", "-X", "GET", &format!("{base}/iserver/auth/status")])
        .output()
        .expect("curl should run");
    assert!(output.status.success(), "curl failed: {output:?}");
    // Decoding is the assertion; `authenticated` depends on live login state.
    let _status: AuthStatus =
        decode(&output.stdout).expect("live auth/status should decode into AuthStatus");
}
```

- [ ] **Step 2: Verify it compiles and is skipped by default**

Run: `cargo test -p oath-adapter-ibkr --test live`
Expected: compiles; `1 test, 0 passed, 1 ignored` (skipped).

Run: `just lint`
Expected: clean.

- [ ] **Step 3: (Optional, manual) run it against the live gateway**

With the gateway up + logged in: `cargo test -p oath-adapter-ibkr --test live -- --ignored`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/adapter/ibkr/tests/live.rs
git commit -m "test(ibkr): gated live-gateway integration test (#[ignore])"
```

---

### Task 11: README, CHANGELOG, and full CI gate

**Files:**
- Modify: `README.md` (crate table, dependency graph, "coming soon" line)
- Modify: `CHANGELOG.md` (`[Unreleased]`)

**Interfaces:**
- Produces: the finished PR — docs updated and `just ci` green.

- [ ] **Step 1: Add the crate table row**

In `README.md`, add after the `oath-adapter-net-ws-api` row:

```markdown
| `oath-adapter-ibkr` | IBKR venue adapter — Client Portal API v1 (`cpapi`) read-path wire layer; `webapi` (beta OAuth) / `tws` (socket) surfaces to follow |
```

- [ ] **Step 2: Add the dependency-graph node**

In the `mermaid` graph in `README.md`, add a standalone node near the other adapter nodes (the crate depends on no internal crate — only `serde`):

```
    ibkr[oath-adapter-ibkr]
```

- [ ] **Step 3: Update the "coming soon" line**

Replace the venue-adapter clause of the closing paragraph:

```markdown
The crates above are compiling skeletons. Bus/Event-Log/persistence backends (e.g. `oath-bus-iceoryx2`, `oath-event-log-chronicle`, `oath-persistence-sqlite`) are coming soon. The first venue adapter, `oath-adapter-ibkr`, has begun with its Client Portal API v1 read-path wire layer.
```

- [ ] **Step 4: Add the CHANGELOG entry**

In `CHANGELOG.md`, insert an `### Added` section immediately under `## [Unreleased]` (above the existing `### Changed`):

```markdown
### Added

- **`oath-adapter-ibkr` (new crate) — IBKR Client Portal API v1 read-path wire layer.**
  Transport-agnostic serde DTOs for the CP API v1 read endpoints (`iserver/auth/status`,
  `tickle`, `iserver/accounts`, `portfolio/accounts`, `portfolio/{acct}/positions`,
  `iserver/secdef/search` / `info`), an `Endpoint` descriptor, and a `decode` entry point.
  Depends only on `serde`/`serde_json`/`thiserror`; no OATH-domain translation yet
  (deferred until `InstrumentId`/`Order` land, per ADR-0003/0025/0026). Ships a
  hand-rolled Client Portal Gateway container (`docker/cpapi/`) and a `just ibkr-capture`
  recipe for paper-account fixtures. Web API (beta OAuth 2.0) and TWS (socket) are future
  sibling modules.
```

- [ ] **Step 5: Run the full CI gate**

Run: `just ci`
Expected: PASS — `fmt fmt-toml typos lint check test deny doc machete gitleaks actionlint shellcheck` all green.

- [ ] **Step 6: Commit**

```bash
git add README.md CHANGELOG.md
git commit -m "docs(ibkr): README crate row + graph node + CHANGELOG entry"
```

- [ ] **Step 7: Open the PR**

Push the branch and open a PR that references the issue (`Closes #N`), summarizing the slice: CP API v1 read-path wire layer + paper gateway harness; no domain translation yet.

---

## Self-Review

**Spec coverage:**
- §2 read endpoints (auth/status, tickle, iserver/accounts, portfolio/accounts, positions, secdef search/info) → Tasks 2, 4–7. ✅
- §3.1 wire module (`Endpoint`, `CpapiError`, narrow DTO surface, no transport, `cpapi` namespace) → Tasks 1–7. ✅
- §3.2 hand-rolled gateway harness + `just ibkr-capture` + sanitized fixtures → Tasks 8, 9. ✅
- §4 fixture-driven TDD + `#[ignore]` live test + `just doc` per task → Tasks 4–7, 9, 10. ✅
- §5 workspace/lint conformance + README update → Global Constraints + Task 11. ✅
- §6 DoD (crate + tests in `just ci`, harness, gated live test, README, CHANGELOG, `just ci` green) → Task 11. ✅
- §7.4 numeric faithful-mirror (String / `i64` / `serde_json::Number`; no `WireNum`) → Tasks 6, 7. ✅
- §7.5 Web API deferred, not pre-coupled → out of scope by construction (no `webapi` code). ✅

**Placeholder scan:** every step ships concrete file content, an exact command, and expected output. Representative fixtures in Tasks 4–7 are explicitly flagged as reconciled to reality in Task 9 (not a placeholder — a deliberate representative-then-real flow).

**Type consistency:** `decode<T>` (Task 3) is the single deserialization entry used verbatim in Tasks 4–7, 10. Re-export names in `cpapi/mod.rs` accrue monotonically and match each module's `pub` items. `conid` is `String` in `SecdefSearchEntry` and `i64` in `Position`/`SecdefInfo` — intentional and asserted. `Method`/`Endpoint` derive only what tests compare (`Method: PartialEq + Eq`); DTOs derive `Debug, Clone, Deserialize` only (no `PartialEq` → no `derive_partial_eq_without_eq`).
