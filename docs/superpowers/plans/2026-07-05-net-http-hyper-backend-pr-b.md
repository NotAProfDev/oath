# net-http hyper backend — PR B (buffering) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `BufferMode`-driven `Buffered` arm to the hyper leaf so a request carrying `BufferMode::Buffer` gets its response body collected to `Bytes` inside the retry boundary, while the default (and explicit `Stream`) path is unchanged.

**Architecture:** A single additive branch in `HyperLeaf::call`. The leaf reads `req.extensions().get::<BufferMode>()` (default `Stream`, ADR-0030 §4); on `Buffer` it collects the `Incoming` body into `Bytes` and wraps it with `ResponseBody::buffered`, on `Stream` it keeps the PR A `ResponseBody::streaming` arm. No signature, associated-type, or layer change — `type Response` was already `http::Response<ResponseBody<HyperBody>>` in PR A, so both arms produce the same type and every resilience layer is untouched.

**Tech Stack:** Rust 2024. `http-body-util` `BodyExt::collect` (already a dependency and import in `leaf.rs`), `oath_adapter_net_http_api::{BufferMode, ResponseBody}` (both already ship, re-exported at that crate's root). Tests reuse PR A's plain-HTTP loopback helpers.

**Spec:** [docs/superpowers/specs/2026-07-05-net-http-hyper-backend-design.md](../specs/2026-07-05-net-http-hyper-backend-design.md) — see [Delivery: two PRs](../specs/2026-07-05-net-http-hyper-backend-design.md), PR B.

**Predecessor:** [PR A (transport)](2026-07-05-net-http-hyper-backend-pr-a.md), merged in #90. This plan branches off `main` *after* #90 landed.

## Global Constraints

- **Edition 2024, MSRV 1.90.** Validate with `just msrv`.
- **No `unsafe`** (`unsafe_code = "deny"` workspace-wide).
- **No `unwrap`/`expect`/indexing in non-test code** (warned) — return `Result`, model errors with `thiserror`. Test code is exempt.
- **`missing_docs` warned** — every `pub` item gets a doc comment. Clippy `all` is **deny-level**; `pedantic`/`nursery` warn. (PR B adds no new `pub` items.)
- **Definition of done = `just ci` passes** (fmt, lint, test, doc, deny, typos). Per net-http rule, run **`just doc`** in each task's checks — `check`/`lint`/`test` miss broken rustdoc intra-doc links.
- **Conventional Commits**, enforced by the `commit-msg` hook; **subject ≤ 72 chars**. End every commit message with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Worktree:** all work in `.claude/worktrees/net-http-hyper-buffer` on branch `feat/net-http-hyper-buffer`, created **off `main` after #90 merged** (`git fetch origin main` first — the primary checkout may still be behind). Never touch the primary checkout's branch.
- **Additive only.** The whole PR is one `match` arm plus tests: no change to `type Response`, `type Error`, `HyperBody`, `ConnConfig`, `hyper_leaf`, `build()`, or any resilience layer. If a change would touch a signature, stop — it is out of scope for this slice.
- **Dependency direction:** unchanged. No new crate dependency (`http-body-util`, `oath-adapter-net-http-api` are already deps; `BodyExt` is already imported in `leaf.rs`).

---

### Task 1: The `BufferMode`-driven `Buffered` arm on the leaf

The one behavioural change: `HyperLeaf::call` branches on the request's `BufferMode` extension. `Buffer` collects the body into `Bytes` and returns a `ResponseBody::buffered`; `Stream` (the default, and the explicit value) returns the PR A `ResponseBody::streaming` arm unchanged. Test-driven: the failing buffered test drives the branch in; a regression test proves the default/`Stream` path is unchanged.

**Files:**
- Modify: `crates/adapter/net/http/hyper/src/leaf.rs` (the `import` line, the `call` body, and the in-crate `tests` module)

**Interfaces:**
- Consumes: `HyperLeaf`, `hyper_leaf`, `ConnConfig` (PR A); `oath_adapter_net_http_api::{BufferMode, ResponseBody, HttpError}`; `crate::error::{map_hyper_err, map_legacy_err}` (PR A); `http_body_util::{BodyExt, Full}` (already imported); the `spawn_echo_server`/`test_conn` helpers already in the `tests` module (PR A, Task 3).
- Produces: no new public items. `HyperLeaf::call` gains the `Buffer` branch; `type Response` is unchanged (`http::Response<ResponseBody<HyperBody>>`).

- [ ] **Step 1: Write the failing buffered-body test**

Add to the `#[cfg(test)] mod tests` block in `crates/adapter/net/http/hyper/src/leaf.rs`. First extend the module's `use` imports (add `BufferMode` and `ResponseBody` — the round-trip helpers `spawn_echo_server`, `test_conn`, and `HyperLeaf`/`hyper_leaf` are already imported by PR A):

```rust
    use oath_adapter_net_http_api::{BufferMode, ResponseBody};
```

Then add the test:

```rust
    #[tokio::test]
    async fn buffer_mode_collects_the_body_into_a_buffered_arm() {
        let base = spawn_echo_server(b"pong").await;
        let leaf = hyper_leaf(test_conn());
        let mut req = http::Request::get(format!("{base}/ping"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Buffer);

        let resp = leaf.call(req).await.expect("round-trip");
        assert!(
            matches!(resp.body(), ResponseBody::Buffered { .. }),
            "BufferMode::Buffer must yield a Buffered arm, got a streaming body"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, Bytes::from_static(b"pong"));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p oath-adapter-net-http-hyper --all-features buffer_mode_collects_the_body_into_a_buffered_arm`
Expected: FAIL — the assertion `matches!(resp.body(), ResponseBody::Buffered { .. })` fails because PR A's `call` always builds `ResponseBody::streaming(...)`. (It compiles: `BufferMode`, `ResponseBody::Buffered`, and `extensions_mut().insert` all already exist.)

- [ ] **Step 3: Add the `BufferMode` branch to `call`**

In `crates/adapter/net/http/hyper/src/leaf.rs`, add `BufferMode` to the production `use` of `oath_adapter_net_http_api` — change:

```rust
use oath_adapter_net_http_api::{HttpError, ResponseBody, Service};
```

to:

```rust
use oath_adapter_net_http_api::{BufferMode, HttpError, ResponseBody, Service};
```

Then replace the body of `HyperLeaf::call`'s `async move` block. The current (PR A) block is:

```rust
        let client = self.client.clone();
        async move {
            let (parts, body) = req.into_parts();
            let req = http::Request::from_parts(parts, Full::new(body));
            let resp = client.request(req).await.map_err(map_legacy_err)?;
            let (parts, incoming) = resp.into_parts();
            let mapper: fn(hyper::Error) -> HttpError = map_hyper_err;
            let body = ResponseBody::streaming(incoming.map_err(mapper));
            Ok(http::Response::from_parts(parts, body))
        }
```

Replace it with (reads the mode before `into_parts` consumes `req`; the `Buffer` arm collects `Incoming` and maps its `hyper::Error` with the same `map_hyper_err`):

```rust
        let client = self.client.clone();
        async move {
            // ADR-0030 §4: absent extension ⇒ Stream. `BufferMode` is `Copy`.
            let mode = req
                .extensions()
                .get::<BufferMode>()
                .copied()
                .unwrap_or(BufferMode::Stream);
            let (parts, body) = req.into_parts();
            let req = http::Request::from_parts(parts, Full::new(body));
            let resp = client.request(req).await.map_err(map_legacy_err)?;
            let (parts, incoming) = resp.into_parts();
            let body = match mode {
                BufferMode::Stream => {
                    let mapper: fn(hyper::Error) -> HttpError = map_hyper_err;
                    ResponseBody::streaming(incoming.map_err(mapper))
                }
                BufferMode::Buffer => {
                    // Collect inside the retry boundary → full-body retry coverage.
                    let bytes = incoming.collect().await.map_err(map_hyper_err)?.to_bytes();
                    ResponseBody::buffered(bytes)
                }
            };
            Ok(http::Response::from_parts(parts, body))
        }
```

The `Stream` arm keeps `ResponseBody<HyperBody>`; the `Buffer` arm's `ResponseBody::buffered` infers the same `B = HyperBody`, so the `match` unifies and `type Response` is unchanged.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p oath-adapter-net-http-hyper --all-features buffer_mode_collects_the_body_into_a_buffered_arm`
Expected: PASS — the response is a `Buffered` arm carrying `pong`.

- [ ] **Step 5: Write the streaming-default regression test**

Prove the change is additive: with no extension (default) *and* with an explicit `BufferMode::Stream`, the body is still the `Streaming` arm. Add to the same `tests` module:

```rust
    #[tokio::test]
    async fn default_and_explicit_stream_keep_a_streaming_body() {
        let base = spawn_echo_server(b"pong").await;
        let leaf = hyper_leaf(test_conn());

        // No BufferMode extension → default Stream (ADR-0030 §4).
        let req = http::Request::get(format!("{base}/a"))
            .body(Bytes::new())
            .unwrap();
        let resp = leaf.call(req).await.expect("round-trip");
        assert!(
            matches!(resp.body(), ResponseBody::Streaming { .. }),
            "absent BufferMode must stay streaming"
        );

        // Explicit BufferMode::Stream → same.
        let mut req = http::Request::get(format!("{base}/b"))
            .body(Bytes::new())
            .unwrap();
        req.extensions_mut().insert(BufferMode::Stream);
        let resp = leaf.call(req).await.expect("round-trip");
        assert!(
            matches!(resp.body(), ResponseBody::Streaming { .. }),
            "explicit Stream must stay streaming"
        );
    }
```

- [ ] **Step 6: Run the full leaf test suite**

Run: `just test`
Expected: PASS — the two new tests plus PR A's existing leaf tests (`leaf_round_trips_a_plain_http_body`, the error-path and TLS tests) all green.

- [ ] **Step 7: Verify lint + docs**

Run: `just lint && just doc`
Expected: PASS — no new warnings; rustdoc links resolve.

- [ ] **Step 8: Commit**

```bash
git add crates/adapter/net/http/hyper/src/leaf.rs
git commit -m "feat(net): hyper leaf BufferMode::Buffer arm"
```

---

### Task 2: CHANGELOG entry + full CI + open the PR

Records the buffering PR in the changelog (the ADR-0030 §7 amendment already landed in PR A — no ADR change here) and closes the PR out against the full gate.

**Files:**
- Modify: `CHANGELOG.md` (`[Unreleased]` → `### Added`)

**Interfaces:**
- Consumes/Produces: none (docs + CI).

- [ ] **Step 1: Add the CHANGELOG entry**

In `CHANGELOG.md` under `## [Unreleased]` → `### Added`, immediately after the existing `**net-http hyper backend (transport).**` bullet, add a sibling bullet:

```markdown
- **net-http hyper backend (buffering).** The hyper leaf now honours the
  per-request `BufferMode` (ADR-0030 §4): `BufferMode::Buffer` collects the
  response body to `Bytes` inside the retry boundary (`ResponseBody::buffered`);
  absent or `Stream` keeps the live streaming body. Additive — no signature,
  associated-type, or layer change. (#<PR-B>)
```

(Replace `#<PR-B>` with the PR number once opened.)

- [ ] **Step 2: Run the full CI gate**

Run: `just ci`
Expected: PASS — fmt, lint, test, doc, deny, typos all green (identical to GitHub Actions). No new dependencies, so `deny` has nothing new to classify.

Run: `just msrv`
Expected: PASS — builds on MSRV 1.90.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(net): changelog for hyper leaf buffering (PR B)"
```

- [ ] **Step 4: Open the issue + PR**

```bash
git push -u origin feat/net-http-hyper-buffer
gh issue create --title "feat(net): hyper leaf buffering — BufferMode::Buffer arm (Slice, PR B)" \
  --label enhancement \
  --body "The hyper-backend slice PR B (follows #90): add the BufferMode-driven Buffered arm to HyperLeaf::call so a request carrying BufferMode::Buffer gets its body collected inside the retry boundary. Additive; no signature/type/layer change. Design: docs/superpowers/specs/2026-07-05-net-http-hyper-backend-design.md (Delivery: two PRs, PR B). Plan: docs/superpowers/plans/2026-07-05-net-http-hyper-backend-pr-b.md"
gh pr create --title "feat(net): hyper leaf buffering (BufferMode::Buffer arm)" \
  --body "Closes #<ISSUE>. Second and final PR of the hyper-backend slice (PR A was #90). Adds the BufferMode::Buffer branch to HyperLeaf::call: collect Incoming → ResponseBody::buffered; default/Stream unchanged. See docs/superpowers/plans/2026-07-05-net-http-hyper-backend-pr-b.md."
```

(Fill `#<ISSUE>` from the created issue; update the CHANGELOG `#<PR-B>` placeholder after the PR number is known, in a follow-up commit if desired.)

---

## Notes for the executor

- **Scope discipline.** PR B is one `match` arm plus two tests plus a changelog line. If you find yourself editing `type Response`, `HyperBody`, `ConnConfig`, `build()`, or any resilience layer, you have left the slice — stop and re-read the spec's "Additive only" line.
- **Why `map_hyper_err` on `collect()`.** `Incoming::collect().await` yields `Result<Collected<Bytes>, hyper::Error>`; the same body-error mapper PR A used for the streaming arm normalizes it to `HttpError`, so a mid-collect failure surfaces as `HttpError::Other` (ADR-0030 §6), consistent with the streaming path.
- **`ResponseBody` variant match in tests.** `ResponseBody`'s `Buffered`/`Streaming` variants are as visible as the (public) enum, and the tests are in-crate, so `matches!(resp.body(), ResponseBody::Buffered { .. })` is the direct observable — no need to drive bytes to infer the arm.
- **No new ADR / no ADR-0030 edit.** Buffering was already the design in ADR-0030 §4 and the slice spec; §7's TLS amendment landed with PR A. PR B touches only `CHANGELOG.md` for docs.
