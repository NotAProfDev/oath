# net-http deep review — best practices, performance & a clean-slate opinion (2026-07-06)

Companion to [Fable's defect audit](2026-07-05-net-http-audit-findings.md). Where that
audit hunts line-level defects, this one answers three questions the user asked:

1. Does the implemented HTTP stack follow **industry best practices**?
2. Are there **performance** problems, in depth?
3. If I were building this **from scratch today with no ADRs**, would I do it differently — and why?

**Method.** A 7-lens multi-agent review (performance, HTTP semantics, resilience prior-art,
API/module design, async correctness, backend/TLS/deps, test strategy) → adversarial
verification of *every* material finding against current `main` (post-#92) → three
independent clean-slate architects (tower-native / minimalist-first-party / domain-first) →
a completeness critic — cross-checked against my own line-by-line read of all three crates,
the `net-api` contract, and ADR-0029/0030/0031/0034. Findings are graded **CONFIRMED**
(traced in source by a second reader) or **PLAUSIBLE** (real concern, not fully traced). The
verification pass also **refuted five overclaims** (see §7) — those are *not* defects.

---

## 0. Bottom line

**The architecture is genuinely strong and should not be rebuilt.** All three clean-slate
architects, reasoning from different starting philosophies, converged on the same verdict:
**fix in place, do not rewrite on tower/reqwest.** The single best decision in the crate —
`Service::call(&self) -> impl Future + Send` (RPITIT, no `poll_ready`, no `async-trait`, no
`dyn`) — is *superior* to the mainstream tower model for this use case, and the domain layers
(fail-closed pacing, boot-time coverage, merged rate/concurrency, permit-riding-the-body,
IBKR-tuned breaker) have no off-the-shelf equivalent.

**Every verified correctness bug lives in one of two narrow places:** hand-rolled
*primitives* (the jitter PRNG, the token-bucket wait loop) and *error-classification glue*
(C1, H1/H2). None live in the domain composition. That is a very healthy failure
distribution — it means the design is right and the defects are localized and cheap to fix.

**One defect is critical and will halt live trading:** **C1** — a purely *local* pacing
rejection trips the *venue-wide* breaker into the ~15-minute penalty box. Confirmed on
current `main` by 5 of 6 lenses independently and by my own trace.

**The most important things Fable did not surface:**

- In the **default `Stream` mode the resilience verdict is committed at header time** — a
  body that fails mid-transfer bypasses Retry *and* the breaker records it as **Success**.
  The common path has the weakest resilience. (HIGH)
- The **production leaf cannot TLS-connect to the IBKR gateway it is built for** —
  `hyper_leaf` hardcodes `with_webpki_roots()`, but IBKR's Client Portal gateway is a
  localhost service with a self-signed cert. The leaf's *own* TLS test bypasses `hyper_leaf`
  to make a custom-root client work. (HIGH-for-its-target)
- **Zero operational metrics.** The stack emits one tracing span per request and nothing
  else — no counters/histograms. A 15-minute venue lockout (the C1 scenario) is invisible to
  ops until orders silently stop flowing. (whole-class gap)
- **No graceful shutdown** — dropping the leaf `RST`s in-flight order submissions, leaving
  venue-side order state ambiguous.

---

## 1. What the design gets right (keep — verbatim, on a rebuild too)

Independently praised by all three clean-slate reviewers:

- **`Service<Req>` as `&self` + RPITIT with backpressure *inside* `call`**
  ([service.rs:25](../../../crates/adapter/net/api/src/service.rs#L25)). Sidesteps tower's
  three worst footguns at once: the `poll_ready`-reserve-then-`call` contract, `&mut self`
  forcing `Clone`+`mem::replace` to share across tasks, and the near-mandatory
  `BoxService`/`BoxFuture` at seams. This is the single best call in the crate.
- **Compile-time monomorphized composition**
  ([compose.rs](../../../crates/adapter/net/api/src/compose.rs)) — one fully-resolved type,
  zero boxing, zero per-call heap alloc; first `.layer()` = outermost. (The verifier
  *refuted* the claim that the resulting per-request mutex/hash costs matter — see §7.)
- **The `Timer` seam** ([timer.rs](../../../crates/adapter/net/api/src/timer.rs)) threaded
  through every timing layer. This is what justifies *not* reaching for tokio-coupled
  `tower::timeout`: the stack stays runtime-neutral (the smol/async-std goal) **and**
  deterministically mock-driven. It earns its keep.
- **Statuses-are-not-errors** — 4xx/5xx flow through as `Ok(Response)`; layers peek
  `status()` read-only ([circuit_breaker.rs:84](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L84)).
  Exactly right for a venue client.
- **Boot-time total pacing coverage** — `RateLimitConfig<K>` must be total over `K::all()`,
  validated into a `BuildError` before any leaf work
  ([rate.rs:158](../../../crates/adapter/net/http/api/src/rate.rs#L158),
  [stack.rs:81](../../../crates/adapter/net/http/api/src/stack.rs#L81)). Turning a forgotten
  endpoint from a live 429→15-min box into a *boot failure* is the correct inversion.
- **Fail-closed / fail-safe directives** — absent `RateScope` is rejected, absent `Retryable`
  disables retry (a forgotten stamp never duplicates a `POST`).
- **The pure, clock-injected `Breaker`** ([circuit_breaker.rs:124-254](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L124))
  — every transition takes `now: Instant`, holds no lock/timer, table-tested with zero async;
  the async shell locks briefly, never across the `await`. Textbook deep-module separation.
- **`Guarded`'s release correctness** — permit released at the earliest of terminal frame /
  mid-stream error / early drop / already-ended-at-construction, well-tested
  ([body.rs](../../../crates/adapter/net/http/api/src/body.rs)).
- **Secret-safe `Tracing` by construction** — records only method/path/status/`ErrorKind`/
  latency, drops the query string, with an actual leak-regression test suite.

---

## 2. Best-practice gaps beyond Fable's audit

> I re-verified Fable's C1/H1/H2/M1–M9/L-series against current `main` myself; they all still
> hold post-#92. This section is the **material additions and elevations**. Full table in §8.

### 2A. Correctness/safety that is also a best-practices failure

- **C1 — local pacing reject trips the venue-wide breaker (CRITICAL).** `HttpError::Throttled`
  is produced *only* locally by `RateLimit` — `max_wait` exhaustion, absent `RateScope`,
  missing key ([rate_limit.rs:241,242,303,326,351](../../../crates/adapter/net/http/api/src/rate_limit.rs#L241)).
  A real venue 429 arrives as `Ok(resp)`. But `classify` maps error-side
  `ErrorKind::Throttled` → `Class::TripNow`
  ([circuit_breaker.rs:79](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L79)),
  and the breaker sits *outside* Retry+RateLimit
  ([stack.rs:84-90](../../../crates/adapter/net/http/api/src/stack.rs#L84)), so that local
  Throttled propagates up (Retry never retries it) and opens the *single global* breaker for
  `throttle_cooldown` (~900s). **One over-pacing event or one forgotten stamp takes the whole
  venue client offline for 15 minutes** — the exact penalty box the proactive limiter exists
  to avoid. Fix: error-side `Throttled` → `Class::Ignored` (it never reached the host); only
  `Ok(status==429)` should `TripNow`.
- **Stream-mode resilience is committed at header time (HIGH, new).** `BufferMode` defaults to
  `Stream` ([leaf.rs:70](../../../crates/adapter/net/http/hyper/src/leaf.rs#L70)). The
  resilience layers see the response as `Ok(Response)` at *headers*, so a body that fails
  mid-transfer never re-enters Retry, and the breaker `record`s **Success** on a response
  that never completed. The common path has the weakest coverage. This is the architectural
  root of which H1/H2 are symptoms; the deepest fix is to move buffering into a layer above
  the leaf (see §5) so body outcomes fall inside the retry/breaker boundary by default.
- **The leaf can't reach IBKR's gateway (HIGH for its stated target, new).** `hyper_leaf`
  hardcodes `.with_webpki_roots()`
  ([leaf.rs:107](../../../crates/adapter/net/http/hyper/src/leaf.rs#L107)) — Mozilla public
  roots only. IBKR's Client Portal gateway is a **localhost service with a self-signed cert**
  (matching `NoAuth` + cookie session, ADR-0034 §1). A webpki-roots-only client rejects it.
  Tellingly, the leaf's own TLS test ([leaf.rs:373-435](../../../crates/adapter/net/http/hyper/src/leaf.rs#L373))
  bypasses `hyper_leaf` and hand-builds a client with a custom `RootCertStore` — the plumbing
  exists but is not exposed. Fix: a `ConnConfig` root-store option (system store / custom
  roots / explicit trust), and a `[features]` seam for it.
- **H1/H2 — post-connect failures are invisible to Retry and the breaker (HIGH).**
  `map_legacy_err` only branches `is_connect()`
  ([error.rs:16-22](../../../crates/adapter/net/http/hyper/src/error.rs#L16)); every
  post-connect reset / cancel / incomplete-message / H2 `GOAWAY`/`RST_STREAM`, and every
  body-phase error via `map_hyper_err`, becomes `HttpError::Other` → `ErrorKind::Unknown` →
  non-transient for Retry ([retry.rs:200](../../../crates/adapter/net/http/api/src/retry.rs#L200))
  and `Ignored` by the breaker. **Stale-connection retry is table-stakes for any pooled HTTP
  client** and it is simply absent here — the leaf tests *enshrine* the gap
  (`aborted_connection_surfaces_an_http_error` asserts `Other`). `BufferMode::Buffer`,
  documented as "full retry coverage," produces a *non-retryable* error on a truncated body —
  the opposite of its purpose. Fix: inspect `hyper::Error::{is_incomplete_message, is_canceled,
  is_body_write_aborted}` + io-error sources and map resets/truncation → `Connection`.
- **Silent cleartext downgrade (MED, N2).** `.https_or_http()`
  ([leaf.rs:108](../../../crates/adapter/net/http/hyper/src/leaf.rs#L108)) permits plaintext
  `http://`, so a misconfigured base URL exfiltrates `Authorization` headers with no error.
  This exists only so plain-HTTP echo tests pass — production should not inherit a test
  convenience. Gate `http` behind config / `cfg(test)`.
- **No HTTP/2 keepalive PING (MED, new).** The pooled client enables h2 but sets no
  `http2_keep_alive_interval`/`_timeout`/`_while_idle`
  ([leaf.rs:113-117](../../../crates/adapter/net/http/hyper/src/leaf.rs#L113)). Idle
  multiplexed connections to a long-lived venue get silently reaped (NAT/LB), and the next
  request eats a reconnect+handshake on the latency path — or, combined with H1, fails
  invisibly. *(§7: whether reaping actually occurs depends on `pool_idle_timeout` vs the
  venue's policy — pin with an integration test before acting.)*
- **No graceful shutdown / pool drain (new, critic).** `HyperLeaf` exposes only `call`; drop
  abandons in-flight requests and `RST`s pooled sockets. For a trading engine, an abrupt drop
  mid-order-submit leaves venue-side state ambiguous (did the order land?). There is no
  `shutdown()`/drain seam.

### 2B. Resilience patterns vs industry (resilience4j / Polly / Finagle / AWS SDK v2 / governor)

- **Consecutive-count breaker is blind to mixed-traffic degradation.** Any interleaved 2xx
  resets the streak ([circuit_breaker.rs:199](../../../crates/adapter/net/http/api/src/circuit_breaker.rs#L199)),
  so a 50%-error host **never trips**. resilience4j/Polly use a rolling error-rate window.
  ADR-deferred ("rolling-window later"), but for a venue this is a real detection hole.
- **"Never hit 429" is not actually guaranteed (MED, new).** The token bucket seeds and
  refills to `burst` ([rate_limit.rs:78-84,293](../../../crates/adapter/net/http/api/src/rate_limit.rs#L78)),
  so it can admit up to `burst + rate·T` inside a server's *sliding* window — a token bucket
  and a sliding-window limiter are not equivalent at the boundary. The proactive guarantee is
  softer than the ADR implies. *(§7: needs a trace against IBKR's exact window definition +
  an integration test — no test exercises the refill path at all.)*
- **No `Retry-After` honoring; retryable-status scoping is off (PLAUSIBLE).** 503/429
  `Retry-After` is ignored (fixed jitter instead); 408/425 are never retried; all 5xx —
  including permanent 501/505 — are retried. ADR-deferred, but for a penalty-box venue,
  server-directed delay is exactly the signal you want to obey.
- **Correlated jitter across clones (elevate L3).** `RetryLayer::layer` seeds every service
  from the same `cfg.seed` ([retry.rs:162](../../../crates/adapter/net/http/api/src/retry.rs#L162))
  and `Clone` snapshots state ([retry.rs:68-75](../../../crates/adapter/net/http/api/src/retry.rs#L68)).
  The ADR's own recommended concurrency pattern (clone the client per spawned task) therefore
  yields **identical** backoff sequences across concurrent tasks → synchronized retries on a
  shared outage — the exact thundering herd full-jitter exists to prevent. The `seed` is
  fixed everywhere shown (`seed: 1`), so nothing enforces the doc's "varied per process."
- **Single global breaker blast radius.** One `Arc<Mutex<Breaker>>` for the whole venue means
  a fault on one endpoint fast-rejects all. Correct for IBKR's IP-wide 429 box, but per-key
  breakers (deferred) would better match mixed rate/concurrency endpoints with independent
  health — and would blunt C1's blast radius.

### 2C. Observability (all new — critic + M2)

- **Zero numeric metrics.** The only telemetry is one span per request; nothing under
  `crates/adapter/net` depends on `metrics`/`prometheus`/`opentelemetry`. You cannot alert on
  or dashboard the very signals this stack exists to manage: breaker Open/Half-Open
  transitions, 429/Throttled rate, retry amplification, permit wait time, concurrency
  saturation. The C1 lockout is invisible to ops.
- **`route` label is the raw URI path → unbounded cardinality.**
  [trace.rs:~130] records `route = uri().path()` verbatim. IBKR paths embed ids
  (`/iserver/account/{acctId}/order/{orderId}`, `.../marketdata/{conid}/history`), so any
  span→metrics exporter sees unbounded label cardinality — ironic next to `error_kind`, which
  is a bounded `&'static str` precisely to avoid this. Needs a route-templating seam.
- **`CircuitOpen` mislabeled "unknown" (M2).** `kind_label` has no `CircuitOpen` arm
  ([trace.rs:30-40](../../../crates/adapter/net/http/api/src/trace.rs#L30)); the single most
  operationally important state is logged as the least informative label.

### 2D. Secret hygiene & API shape

- **Derived `Debug` leaks secrets (M3).** `Auth` ([auth.rs:43](../../../crates/adapter/net/http/api/src/auth.rs#L43)),
  `SetHeaders` ([auth.rs:86](../../../crates/adapter/net/http/api/src/auth.rs#L86)), and
  `HttpConfig` ([stack.rs:29](../../../crates/adapter/net/http/api/src/stack.rs#L29)) all
  `#[derive(Debug)]` over a `HeaderMap`/`AuthSource` that can hold API keys/tokens — while
  *every other layer* hand-writes a redacting `finish_non_exhaustive`. One `debug!(?config)`
  dumps credentials. The odd ones out.
- **`Body: Send` missing from the return bound (M5).** `stack()`/`build()` return
  `impl HttpClient + Clone + Send + Sync + 'static` but leave the associated `Body`
  unbounded, so response bodies can't cross `tokio::spawn` — the stack's *own* test works
  around it with `spawn_local`+`LocalSet` and a paragraph of explanation
  ([stack.rs:463-484](../../../crates/adapter/net/http/api/src/stack.rs#L463)). The concrete
  `HyperBody` *is* `Send`, so `impl HttpClient<Body: Send>` costs nothing today.
- **`RateScope` makes illegal states representable (M6).** `{ scope: Scope, key: Option<K> }`
  allows `Local`/`Both` + `key: None`, caught only at runtime by fail-closed `Throttled` —
  which (pre-C1-fix) then trips the breaker. `enum RateScope<K> { None, Global, Local(K),
  Both(K) }` makes it a compile-time impossibility.
- **`ResponseBody` leaks its machinery (M9).** `pub enum` with `pub` `Buffered`/`Streaming`
  variants exposing `Full<Bytes>` ([body.rs:32](../../../crates/adapter/net/http/api/src/body.rs#L32),
  `#[allow(missing_docs)]`); adapters can match on / construct the internal representation the
  module documents itself as hiding. *(Self-verified — the adversarial verifier for this one
  hit a transient connection error.)*

### 2E. Dependency / config / deployment hygiene

- **`tokio` `features=["full"]` in production (M7).** Workspace-pinned
  ([Cargo.toml:83](../../../Cargo.toml#L83)) and inherited by the prod hyper leaf, dragging
  `fs`/`process`/`signal`/`io-std` into every downstream binary that needs ~`rt`/`net`/`time`.
- **`ring` compiled alongside `aws-lc-rs` (L5-adjacent).** The single-crypto-backend posture
  is defeated by a transitive `ring`, and nothing bans it via `cargo-deny`.
- **Asymmetric config validation (new, critic).** Pacing params are meticulously validated
  (`rate`/`per`/`burst`/`max` ≠ 0), but `timeout`/`cooldown`/`throttle_cooldown`/
  `rate_limit_max_wait` are unchecked `Duration`s. `timeout = ZERO` makes every send instantly
  `Timeout`; `cooldown = ZERO` defeats the reactive breaker. Validation investment is lopsided.
- **No outbound-proxy support (new, critic).** `HttpConnector::new()` has no
  env-proxy/CONNECT-tunnel awareness — deployment-blocking behind a mandatory corporate proxy,
  common in regulated finance.
- **No worked example/onboarding docs (new, critic).** No `examples/` or README under
  `crates/adapter/net`. Correct use requires manually stamping `RateScope`/`Retryable`/
  `BufferMode` on *every* request; the protocol is documented only inside test modules. A
  forgotten `RateScope` fails closed → (via C1) trips the venue-wide breaker. **This is the
  most likely real-world trigger of the C1 lockout.**

---

## 3. Performance, in depth

The composition itself is fast: fully monomorphized, no `dyn`, no per-call box, locks never
held across `await`. The real per-request costs, worst-first:

- **Token-bucket wait loop (low; L11).** `acquire_rate` is a compute-wait-recheck loop with no
  reservation and no queue ([rate_limit.rs:288-306](../../../crates/adapter/net/http/api/src/rate_limit.rs#L288)):
  a token is decremented only when a waiter *wins* the (unfair `std::sync::Mutex`) lock, never
  reserved before sleeping. N burst waiters compute ~identical sleeps, wake correlated,
  re-lock, one wins, the rest recompute+re-sleep → superlinear wakeups and no FIFO fairness (a
  late arrival can starve an early one to `max_wait`). **Bounded in practice** because the
  limiter is proactive (self-paced to ~10 req/s, so N stays small) and the failure mode is a
  retryable `Throttled`, not corruption — hence graded *low*. A GCRA/virtual-scheduling
  limiter (single `AtomicU64` TAT, one exact sleep per request, lock-free fast path) is the
  clean fix and also removes the per-iteration float math.
- **Per-request `Vec` allocations (M8).** `acquire` heap-allocates two `Vec<&Bucket>` per
  request for at most 2 buckets ([rate_limit.rs:230-231](../../../crates/adapter/net/http/api/src/rate_limit.rs#L230)).
  Replace with `[Option<&Bucket>; 2]` / inline `match`.
- **Unconditional request clone in Retry (L2).** `self.inner.call(req.clone())`
  ([retry.rs:244](../../../crates/adapter/net/http/api/src/retry.rs#L244)) clones the whole
  `Request` (HeaderMap + Extensions box-per-entry, not just the cheap `Bytes` refcount) on
  *every* attempt — including the final attempt and even when `!eligible` (one send). Clone
  only when another attempt may follow; move the owned `req` on the last send.
- **Buffered concurrency permit held until drain (M4).** Under `BufferMode::Buffer` the leaf
  fully collects the body (transfer done), but `RateLimit` still wraps it in `Guarded` with
  the permit ([rate_limit.rs:353](../../../crates/adapter/net/http/api/src/rate_limit.rs#L353));
  `Full<Bytes>` reports `is_end_stream()==false` until polled, so the permit rides an
  *in-memory* body until the caller drains it — serializing decode behind a scarce
  concurrency slot (`/history` max=5). `body.rs:95` even *claims* "permit: None for buffered"
  — a guarantee the code can't honor because `RateLimit` never reads `BufferMode`.
- **Correlated jitter (perf angle of L3).** See §2B — clones replay identical backoff.
- **Blocking DNS per cold connection (new, critic).** `HttpConnector::new()` uses hyper-util's
  `GaiResolver` — blocking `getaddrinfo` on the shared blocking pool, no caching, no
  happy-eyeballs/IPv6 policy ([leaf.rs:101](../../../crates/adapter/net/http/hyper/src/leaf.rs#L101)).
  A real, unmeasured tail-latency source on every new connection.
- **Per-call hyper `Client` clone (L4, trivial).** [leaf.rs:63](../../../crates/adapter/net/http/hyper/src/leaf.rs#L63)
  — cheap `Arc` bump, avoidable but negligible.
- **Unbounded buffered `collect()` (N1).** [leaf.rs:82](../../../crates/adapter/net/http/hyper/src/leaf.rs#L82)
  — a misbehaving venue can OOM the process; cap via `size_hint().upper()` + enforce while
  collecting.

**Not a problem (verifier-refuted overclaims):** the default-SipHash bucket lookup (negligible
vs the work around it), the two brief breaker mutex acquisitions per request (fine), and the
"retry-storm amplification" concern (retry is opt-in and never touches POST/429/4xx). See §7.

---

## 4. Test-strategy gaps (extends M10, all verified)

- The **proactive wait+refill loop — the limiter's defining feature — has ZERO coverage**;
  every rate test uses `max_wait=0`, so the sleep-then-reacquire path is never exercised.
- **Refill *rate* is under-asserted** — a 2× or off-by-one refill bug passes every test.
- **`Scope::Both`** is never driven through `acquire()` (global+local, rate-then-concurrency
  ordering untested).
- **Half-Open + `TripNow` re-trip** (429 during a probe → `throttle_cooldown`) is unpinned.
- **RateLimit-outside-Timeout ordering** is asserted only in a *comment* — swapping the layers
  passes the suite.
- **No C1 regression test** — a one-line "local Throttled must not trip the breaker" would
  have caught it.
- **`stack()` is never integration-tested over the real hyper leaf** — H1/H2 consequences are
  reasoned about, not observed.
- **No `SplitMix64` golden-vector test** (algorithm drift breaks deterministic replay
  silently); **no loom/concurrency tests** for the shared `Mutex<TokenState>` / `Arc<Mutex<Breaker>>`;
  **Retry backoff schedule not pinned**.
- **loom deferred (PR8/#101, 2026-07-08):** no loom model for `Mutex<TokenState>` /
  `Arc<Mutex<Breaker>>`. Both locks are held only across tiny non-`await` critical
  sections; a loom test adds little now. Revisit if a lock scope grows to span an
  `await`. Tracked as a Tier-2 candidate.
- **h2-keepalive reaping (negative case) deferred (PR8/#101, 2026-07-08):** Task 10
  (`hyper/src/leaf.rs`) shipped only the POSITIVE h2-keepalive survival test — an
  idle pooled h2 connection with keepalive enabled stays usable. The NEGATIVE case
  (an idle connection REAPED when keepalive is disabled) depends on hyper's/OS
  idle-connection timing and is flake-prone as a unit test; deferred deliberately.
  Tracked as a Tier-2 candidate.

---

## 5. Would I build it differently? (the clean-slate answer)

Three architects reasoned independently from three philosophies. **They converge:** the spine
is right; a tower/reqwest rebuild would be *strictly worse* here; the work is a targeted
re-cut of a few over-fused seams, not a rewrite.

**No — I would not rebuild on tower + tower-http + governor.** The `&self`+RPITIT `Service`,
the compile-time `Layer` spine, the `Timer` seam, statuses-as-`Ok`, boot-time coverage, the
fail-closed/fail-safe directives, the pure clock-injected breaker, and the untyped-`Bytes`
transport seam are all correct and mostly have no off-the-shelf equivalent. tower's `poll_ready`
+ `&mut self` + boxed-futures ergonomics are worse for a latency-sensitive, runtime-neutral,
mock-driven trading client. Keep the transport cut too — `hyper-util` + `hyper-rustls` +
`aws-lc-rs`, *not* reqwest (whose redirect/cookie/decompress batteries duplicate the stack).

**Yes — on a greenfield I would re-cut four over-fused seams** (each of which spawned a real
bug), and I would delete the reinvented low-level primitives:

1. **Type the error so "I locally declined" can never be read as "the venue said no."** A flat
   `HttpError`/`ErrorKind` conflates `Throttled`/`CircuitOpen`/pre-send `Timeout` (local
   decisions) with transport failures — the direct cause of C1. Model a "reached the venue?"
   distinction (two variants, or a `LocalReject` vs `Transport` split). Bonus: `classify`
   becomes *total* (today its `Server`/`Client` arms and `kind_label`'s `Client`/`Server` arms
   are dead — nothing maps there).
2. **Replace `http::Extensions` as the directive channel with a typed request envelope.**
   `RateScope`/`Retryable`/`BufferMode`/`RequestTimeout` ride as a runtime typemap, re-`get`
   per layer per request, with the mandatory `RateScope` enforced only by a runtime fail-closed
   `Throttled`. A `struct Directives { scope, retry, buffer, timeout }` makes the mandatory
   field compiler-enforced (kills the whole "forgot to stamp → runtime Throttled → C1" class)
   and removes the per-request downcasts. *Tradeoff:* you lose the tidy `HttpClient` blanket
   impl and tower interop — acceptable here because the stack owns both ends (adapter builds
   the request, leaf consumes it).
3. **Split the merged `RateLimit(rate+concurrency)` into two layers.** A token bucket has no
   lifetime; a concurrency permit lives across the streamed body. Fusing them is why `Guarded`
   leaks into `body.rs`, why `acquire` allocs two `Vec`s, why `validate_concurrency_singleton`
   exists, and why a `Both` request can't hold global+local concurrency. Split →
   allocation-free wrapper-free rate limiting, localized permit lifetime, relaxed singleton.
4. **Hoist buffering out of the leaf into a `BufferBody` layer above it (inside `Timeout`).**
   Buffering is cross-backend, and its error classification (the H2 bug) is easy to get wrong
   *because* it lives in each leaf. One layer → every backend streams-only, one audited
   error-mapping point, and body outcomes fall inside the retry/breaker boundary by default
   (fixes the stream-mode-commits-at-headers gap), plus it can bound a stalled `collect`.

Plus: **delete the hand-rolled primitives** — `SplitMix64` → `fastrand` (audited, two lines;
also decorrelate jitter per-call so clones diverge); the token-bucket loop → a GCRA/`governor`
-style limiter. Every reinvented primitive bought a bug and no functionality.

**One genuinely debatable reinvention:** `compose.rs` is an almost line-for-line
reimplementation of `tower::{Layer, Stack, Identity, ServiceBuilder}`, and `tower::Layer` is
*Service-agnostic* — so the bespoke `Service` trait does **not** force a bespoke compositor.
You could depend on tower solely for `Layer`/`Stack`/`Identity`. Keeping ~140 lines to avoid a
pre-1.0 semver coupling in a runtime-neutral contract crate is *defensible*, but it should be a
**documented deliberate choice**, not incidental.

**And I would add what a trading engine needs and this doesn't have** (the domain-first lens):
an operational **metrics plane**; **graceful shutdown**; **symmetric config validation**;
**request priority/fairness** (an order-cancel must be able to preempt a burst of market-data
GETs draining the shared global bucket — today admission order is effectively random);
**bounded backpressure to Core** (today requests park up to `rate_limit_max_wait` — "minutes"
at IBKR's 1/15-min buckets — with no cap on parked count and no fast "we're saturated" signal);
and **connection seams** (root-store, proxy, DNS caching).

**Verdict:** on the *existing* code — **fix in place** (Fable's 6-PR plan + the additions in
§6). On a true greenfield — **significant-refactor of those four seams**, same bones. Either
way, **do not rebuild on tower/reqwest**: it throws away the best decisions and gains nothing.

---

## 6. Suggested additions to Fable's fix plan

Fable's [6-PR plan](2026-07-05-net-http-audit-findings.md#suggested-fix-plan) is sound. Additions,
by priority:

- **PR 0 (new, blocks the stated target): TLS reachability + security.** `ConnConfig`
  root-store option (system / custom roots) so the leaf can reach IBKR's self-signed gateway;
  `https_only` (gate plaintext behind `cfg(test)`/flag); a `[features]` seam for crypto/roots/
  http-version. Without this the production leaf can't connect to the venue it's built for.
- **PR 1 (breaker + telemetry — Fable's PR1):** C1 `classify` fix + `circuit_open` label + M1
  probe-only guard + checked `Instant` arithmetic. **Add:** metrics counters for breaker
  transitions and throttle rejections.
- **PR 2 (hyper error mapping — Fable's PR2):** H1/H2 connection-class mapping; also mitigates
  the stream-mode gap. **Add:** the `BufferBody`-layer refactor is the deeper fix if pursued.
- **PR 3 (hygiene — Fable's PR3):** M3 Debug, M7 tokio trim, dead dev-dep. **Add:** ban `ring`
  in `cargo-deny`; add HTTP/2 keepalive PING config.
- **New PR: operability.** Metrics plane; graceful `shutdown()`/drain; symmetric
  `HttpConfig`/`CircuitBreakerConfig` validation; a worked `examples/` + README for the
  extension protocol.
- **Perf PR (Fable's PR5):** M8 slots + L2 conditional clone + L3 `fastrand`/decorrelated
  jitter. **Add:** consider the GCRA limiter and a bounded-parking admission cap; a DNS-caching
  resolver seam.

## 7. Overclaims the verification pass refuted (not defects)

Recorded for honesty — the adversarial verifiers killed these:

- **"Default SipHash on the bucket map is a hot-path cost."** True that `HashMap` uses SipHash,
  but it's tens of ns and dwarfed by the surrounding work; not a defect. → **info**.
- **"Every request serializes on the global breaker/bucket mutex — unaffordable contention."**
  The locks are held only for the tiny admit/record blocks, never across `await`; fine for
  this throughput. → **info**.
- **"No retry budget → Nx load amplification during outages."** Retry is opt-in (`Retryable`)
  and never touches POST/429/4xx/Auth; combined with proactive pacing, no real amplification.
  → **low**.
- **"Shared mocks can't reproduce leaf failure modes → downstream tests blind."** Real that
  `MockClient`/`MockBody` can't script errors, but they're canned-response doubles by design;
  the inline `ScriptLeaf` doubles cover sequencing. → **info**.
- (Plus one duplicate of H1 raised by a second lens, deduped.)

## 8. Appendix — verified findings (CONFIRMED / PLAUSIBLE, deduplicated)

| Fable | Category | Severity → verified | Verdict | Finding |
|---|---|---|---|---|
| C1 | correctness | critical → critical | CONFIRMED | Local-only `Throttled` trips the venue-wide breaker for the 15-min penalty box |
| — | design | high → high | CONFIRMED | Stream mode commits the resilience verdict at header time — body failures bypass Retry and record as breaker Success |
| — | correctness | high → med | CONFIRMED | Hardcoded `webpki-roots`, no custom-root seam — leaf can't TLS-connect to IBKR's self-signed gateway |
| H1/H2 | correctness | high → high | CONFIRMED | Post-connect/body failures → `Other`/`Unknown` — non-retryable and breaker-invisible; Buffer mode's "full retry coverage" is defeated |
| M5 | design/BP | high → high | CONFIRMED | `stack()`/`build()` return omits `Body: Send` — responses can't cross `tokio::spawn` |
| M3 | correctness | high → med | CONFIRMED | `Auth`/`SetHeaders`/`HttpConfig` derived `Debug` can render secret header values |
| — | design | high → med | CONFIRMED | Pooled h2 client sets no keepalive PING — idle venue connections silently reaped |
| — | design | high → med | CONFIRMED | "Never hit 429" not guaranteed — burst admits `burst + rate·T` in a server sliding window (pin w/ test) |
| M4 | correctness | high → med | CONFIRMED | Buffered concurrency response holds the venue permit until caller drains the in-memory body (contradicts its own doc) |
| M1 | correctness | high → med | CONFIRMED | `ProbeGuard` armed for every admitted call — a cancelled non-probe reopens someone else's Half-Open episode |
| M10 | testing | high → med | CONFIRMED | Proactive wait+refill loop (the limiter's defining feature) has zero coverage; `Scope::Both`, Half-Open re-trip, layer-order all untested |
| N2 | correctness | med → med | CONFIRMED | Connector allows silent cleartext downgrade — misconfigured `http://` exfiltrates `Authorization` |
| M6 | design | med → med | CONFIRMED | `RateScope` makes `Local/Both` + `key:None` representable (runtime-checked, not by construction) |
| M7 | BP | med → med | CONFIRMED | `tokio features=["full"]` dragged into the prod leaf |
| M2 | testing/obs | med → med | CONFIRMED | `kind_label` has no `CircuitOpen` arm — fast-rejects logged as "unknown" |
| — | obs | med → med | CONFIRMED* | Zero numeric metrics; `route` label = raw path → unbounded cardinality *(critic)* |
| — | design | high → low | CONFIRMED | Consecutive-count breaker blind to mixed-traffic degradation (any 2xx resets the streak) |
| L3 | performance | med → low | CONFIRMED | Correlated full-jitter across `Service` clones (same seed + state snapshot) |
| L2 | performance | med → low | CONFIRMED | Retry clones the whole `Request` unconditionally (final attempt / ineligible too) |
| M8 | performance | med → low | CONFIRMED | `acquire` heap-allocs two `Vec`s per request for ≤2 buckets |
| L11 | performance | med → low | CONFIRMED | Token-bucket compute-wait-recheck loop — wakeup storms, no fairness (bounded by proactive pacing) |
| N1 | BP | med → low | CONFIRMED | Buffer mode `collect()` is unbounded — OOM exposure |
| L5 | BP | med → low | CONFIRMED | `ring` compiled alongside `aws-lc-rs`; not banned |
| — | design | med → low | CONFIRMED | No stall timeout on streaming permit / buffered body (slow-body wedges a concurrency slot) |
| — | performance | med → low | CONFIRMED | No `Accept-Encoding`/decompression anywhere |
| — | correctness | med → low | CONFIRMED | No redirect policy; 3xx classified as breaker Success — a session-expiry 302 reads as healthy |
| — | design | med → low | PLAUSIBLE | `Retry-After` unparsed; 408/425 never retried; permanent 5xx retried |
| — | design | med → low | PLAUSIBLE | `http::Extensions` directive channel: silent wrong-`K` fail-closed, no discoverable directive type |
| — | design/ops | — | CONFIRMED* | No graceful shutdown; asymmetric config validation; no proxy support; blocking DNS; no example/docs *(critic)* |
| M9 | design | med → — | self-verified | `ResponseBody` public variants leak `Full<Bytes>`/machinery (verifier errored; confirmed by direct read) |

\* surfaced by the completeness critic (no per-item adversarial verdict; verified by direct read).
