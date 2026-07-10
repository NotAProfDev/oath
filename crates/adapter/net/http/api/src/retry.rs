//! The `Retry` resilience layer (ADR-0031 §2): order-safe retry.
//!
//! Re-issues an **explicitly-eligible** request (a [`Retryable`] marker
//! extension — **absent → never retried**, so a forgotten stamp never
//! duplicates a `POST`) on a **transient** failure (`HttpError::{Timeout,
//! Connection}`) or a `5xx` response, with capped-exponential **full-jitter**
//! backoff up to [`RetryConfig::max_attempts`]. A 429/other 4xx, an `Auth`
//! error, or an `Other` error is **never** retried; on exhaustion the last
//! outcome is returned verbatim. **Body-transparent:** the response body is
//! returned untouched (a superseded response is dropped, releasing any
//! `Guarded` permit). `Auth`/`RateLimit` re-run per attempt because they sit
//! *inside* `Retry`. Runtime-neutral: generic over
//! [`Timer`], jitter via an internal seeded
//! `SplitMix64` (no `rand` dependency).

use crate::{HttpError, Service};
use bytes::Bytes;
use oath_adapter_net_api::{ErrorKind, HasErrorKind, Layer, Timer};
use std::fmt;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A marker `http::Request` extension: its **presence** opts the request into
/// retry (ADR-0031 §2). `Copy` so it survives the per-attempt request clone.
///
/// Eligibility is **explicit-only and fail-safe**: an **absent** marker means
/// the request is sent exactly once and its outcome returned verbatim — a
/// forgotten stamp disables retry, it never duplicates a non-idempotent `POST`.
/// This tightens ADR-0031 §2's "retry idempotent *methods*" default into
/// adapter-stamped intent, the same structural-safety move ADR-0034 Amendment #1
/// made for `RateScope` (see ADR-0034 Amendment #8).
#[derive(Debug, Clone, Copy)]
pub struct Retryable;

/// The `Retry` layer's schedule, as plain `Copy` data.
///
/// `max_attempts` is the **total** number of sends (retries = `max_attempts − 1`);
/// `NonZeroU32` makes "at least one send" a type invariant, so
/// `RetryLayer::new` needs no `Result`. Backoff before the
/// `n`-th retry draws a full-jitter delay from `[0, min(cap, base·2ⁿ⁻¹)]`; `seed`
/// seeds the jitter PRNG (varied per process in production, fixed in tests).
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Total sends allowed for one logical request (retries = this − 1).
    pub max_attempts: NonZeroU32,
    /// The first backoff ceiling — the `n = 1` retry draws its delay from `[0, base]`.
    pub base: Duration,
    /// The exponential-ceiling clamp — no backoff ceiling exceeds this.
    pub cap: Duration,
    /// The jitter PRNG seed (deterministic given seed + draw order).
    pub seed: u64,
}

/// A small [SplitMix64](https://prng.di.unimi.it/splitmix64.c) PRNG for backoff
/// jitter — deterministic given its seed and draw order.
///
/// Lock-free: the 64-bit state advances by the `SplitMix64` step constant via
/// `AtomicU64::fetch_add`, so `duration_in` takes `&self` and holds **no** lock
/// across the backoff `await` (the future stays `Send`). Not cryptographic —
/// full-jitter backoff needs a spread, not uniformity guarantees.
///
/// `Clone` **decorrelates**: a clone is seeded from the parent's advanced+finalized
/// state, not a snapshot, so the ADR's clone-per-task concurrency pattern does not
/// replay identical backoff across tasks (L3).
#[derive(Debug)]
pub(crate) struct SplitMix64 {
    state: AtomicU64,
}

impl Clone for SplitMix64 {
    fn clone(&self) -> Self {
        // Decorrelate clones (L3): advance the parent one step and seed the child
        // from the *finalized* result, so two clones of one generator (the per-task
        // pattern) diverge instead of replaying the same sequence. Still no `rand`
        // dependency — the perturbation is one `SplitMix64` step + finalizer.
        let advanced = self
            .state
            .fetch_add(Self::STEP, Ordering::Relaxed)
            .wrapping_add(Self::STEP);
        Self::new(Self::finalize(advanced))
    }
}

impl SplitMix64 {
    /// The `SplitMix64` stepping constant (fractional bits of the golden ratio).
    const STEP: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Seed the generator.
    pub(crate) const fn new(seed: u64) -> Self {
        Self {
            state: AtomicU64::new(seed),
        }
    }

    /// The `SplitMix64` output finalizer — a fixed bijective bit-mix.
    const fn finalize(mut z: u64) -> u64 {
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Advance the state and return the next 64-bit draw (`SplitMix64` finalizer).
    fn next_u64(&self) -> u64 {
        // `fetch_add` returns the *old* state; add STEP to get the new one — so a
        // fresh generator's first draw finalizes `seed + STEP`, as the reference does.
        let z = self
            .state
            .fetch_add(Self::STEP, Ordering::Relaxed)
            .wrapping_add(Self::STEP);
        Self::finalize(z)
    }

    /// A uniform `Duration` in `[0, ceil]` — one full-jitter sample.
    pub(crate) fn duration_in(&self, ceil: Duration) -> Duration {
        // `ceil` comes from `backoff_ceiling` (≤ `cap`); clamp its nanos into u64
        // (a `cap` above ~584 years is not a real config — clamp, don't panic).
        let ceil_nanos = u64::try_from(ceil.as_nanos()).unwrap_or(u64::MAX);
        // Uniform and **inclusive** over `[0, ceil_nanos]`. `checked_add(1)` is the
        // modulus for every representable ceiling; at the `u64::MAX` edge there is no
        // `MAX + 1` modulus, but the bare `u64` draw already spans `[0, u64::MAX]`
        // inclusively, so no value is lost. (`ceil_nanos == 0` → `% 1` → `ZERO`.)
        // Modulo bias is irrelevant for backoff jitter.
        let pick = ceil_nanos
            .checked_add(1)
            .map_or_else(|| self.next_u64(), |modulus| self.next_u64() % modulus);
        Duration::from_nanos(pick)
    }
}

/// The `Retry` [`Layer`] factory: holds the schedule + clock and produces a
/// [`Retry`] around any inner service.
pub struct RetryLayer<T> {
    cfg: RetryConfig,
    timer: T,
}

impl<T> RetryLayer<T> {
    /// Build the layer from a schedule and a [`Timer`] clock.
    ///
    /// **Infallible** — `RetryConfig::max_attempts` is `NonZeroU32` (≥ 1 send is a
    /// type invariant) and `cap < base` is harmless (the ceiling just never grows
    /// past `cap`), so there is nothing to validate (contrast `RateLimitLayer::new`).
    ///
    /// # Example
    /// ```
    /// use oath_adapter_net_http_api::{RetryLayer, RetryConfig};
    /// use oath_adapter_net_mock::MockTimer;
    /// use std::num::NonZeroU32;
    /// use std::time::Duration;
    ///
    /// let cfg = RetryConfig {
    ///     max_attempts: NonZeroU32::new(3).unwrap(),
    ///     base: Duration::from_millis(50),
    ///     cap: Duration::from_secs(1),
    ///     seed: 1,
    /// };
    /// let _layer = RetryLayer::new(cfg, MockTimer::new());
    /// ```
    #[must_use]
    pub const fn new(cfg: RetryConfig, timer: T) -> Self {
        Self { cfg, timer }
    }
}

impl<T: Clone> Clone for RetryLayer<T> {
    fn clone(&self) -> Self {
        Self {
            cfg: self.cfg,
            timer: self.timer.clone(),
        }
    }
}

impl<T> fmt::Debug for RetryLayer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryLayer")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl<S, T: Clone> Layer<S> for RetryLayer<T> {
    type Wrapped = Retry<S, T>;

    fn layer(&self, inner: S) -> Retry<S, T> {
        Retry {
            inner,
            cfg: self.cfg,
            timer: self.timer.clone(),
            rng: SplitMix64::new(self.cfg.seed),
        }
    }
}

/// The `Retry` middleware: re-issues an eligible request on failure.
///
/// Retries a transient error or a `5xx` response, with capped-exponential
/// full-jitter backoff. Body-transparent — the inner `http::Response<B>` is
/// returned untouched.
pub struct Retry<S, T> {
    inner: S,
    cfg: RetryConfig,
    timer: T,
    rng: SplitMix64,
}

impl<S: Clone, T: Clone> Clone for Retry<S, T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            cfg: self.cfg,
            timer: self.timer.clone(),
            rng: self.rng.clone(),
        }
    }
}

impl<S, T> fmt::Debug for Retry<S, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Retry")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

/// Transient (worth retrying) error kinds — a dropped/timed-out send, not a
/// server verdict. `Throttled`/`Auth`/`Client`/`Server`/`Unknown` are terminal.
const fn is_transient(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::Timeout | ErrorKind::Connection)
}

/// The full-jitter ceiling before the retry that follows a **1-based** `attempt`:
/// `min(cap, base · 2^(attempt-1))`, saturating — no `Duration` overflow reaches
/// the caller (`checked_mul` → `cap`), no shift overflow (`shift` capped at 31).
fn backoff_ceiling(base: Duration, cap: Duration, attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(31);
    let factor = 1u32 << shift;
    base.checked_mul(factor).unwrap_or(cap).min(cap)
}

/// Emit the per-attempt debug event — nests under Tracing's span when present, a
/// no-op otherwise (ADR-0031 §6). `debug`: drill-down, pay-per-use.
fn record_attempt<B>(outcome: &Result<http::Response<B>, HttpError>, attempt: u32) {
    match outcome {
        Ok(resp) => tracing::event!(
            tracing::Level::DEBUG,
            attempt = u64::from(attempt),
            status = u64::from(resp.status().as_u16()),
            "http.attempt"
        ),
        Err(e) => tracing::event!(
            tracing::Level::DEBUG,
            attempt = u64::from(attempt),
            error_kind = crate::trace::kind_label(e.kind()),
            "http.attempt"
        ),
    }
}

impl<S, T, B> Service<http::Request<Bytes>> for Retry<S, T>
where
    S: Service<http::Request<Bytes>, Response = http::Response<B>, Error = HttpError> + Sync,
    T: Timer,
    // `outcome: Result<Response<B>, HttpError>` is dropped before the backoff
    // `.await`, but rustc's generator-interior analysis conservatively unions
    // live ranges around the enclosing `loop`'s back-edge for an unconstrained
    // generic, so it still requires `B: Send` to prove the whole future `Send`
    // (`RateLimit`'s `Service` impl carries the same bound). No `Body` bound —
    // `Retry` still never polls the body.
    B: Send,
{
    type Response = http::Response<B>;
    type Error = HttpError;

    // Not `async fn`: the trait requires the returned future to be `Send`.
    #[allow(clippy::manual_async_fn)]
    fn call(
        &self,
        req: http::Request<Bytes>,
    ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
        async move {
            let eligible = req.extensions().get::<Retryable>().is_some();
            let max = self.cfg.max_attempts.get();
            let mut attempt: u32 = 1;
            // Bounded route label for the retry-amplification metrics (computed once;
            // `req` is moved into the terminal send below).
            let route = crate::meter::route_label(&req);
            // `Request<Bytes>` is `Clone` (`http::Extensions` requires it on insert;
            // `Bytes` is a cheap refcount bump and the directives ride along), and
            // `Auth`/`RateLimit` re-run inside this call so credentials/budget
            // refresh per attempt. The clone is made only when another attempt may
            // follow — the terminal or only send moves the owned request (L2).
            loop {
                let may_retry = eligible && attempt < max;
                if !may_retry {
                    // Terminal send: no further attempt is possible (ineligible, last
                    // attempt, or `max == 1`), so move the owned request — no clone (L2).
                    let outcome = self.inner.call(req).await;
                    record_attempt(&outcome, attempt);
                    // Record the final attempt count onto the current span — the
                    // "http.request" span when composed under Tracing; a no-op
                    // otherwise (no active span / the field is absent).
                    tracing::Span::current().record("attempts", u64::from(attempt));
                    return outcome; // success, non-retryable outcome, or attempts exhausted
                }
                // Another attempt may follow — clone so `req` survives for a retry.
                let outcome = self.inner.call(req.clone()).await;
                record_attempt(&outcome, attempt);
                let retry = match &outcome {
                    Err(e) => is_transient(e.kind()),
                    Ok(resp) => resp.status().is_server_error(), // 5xx only; 429 is 4xx
                };
                if !retry {
                    tracing::Span::current().record("attempts", u64::from(attempt));
                    return outcome; // success or a non-retryable verdict
                }
                // Honor a delay-seconds `Retry-After` on the retryable 5xx as a backoff
                // FLOOR (ADR-0031 Amendment #2): read it before the response is dropped.
                let honored = outcome
                    .as_ref()
                    .ok()
                    .and_then(|resp| crate::retry_after::parse_retry_after(resp.headers()));
                drop(outcome); // release the prior response's Guarded permit before waiting
                let jittered =
                    self.rng
                        .duration_in(backoff_ceiling(self.cfg.base, self.cfg.cap, attempt));
                // Server value overrides local backoff (never re-jittered), floored by
                // our own jittered schedule, and capped by `RetryConfig::cap`.
                let delay = honored.map_or(jittered, |ra| ra.min(self.cfg.cap).max(jittered));
                if honored.is_some() {
                    crate::meter::retry_after_honored("retry");
                }
                // One retry is now committed (a send beyond the first): count it and
                // record its backoff — the retry-amplification signals (deep review §2C).
                crate::meter::retry_attempt(route.clone());
                crate::meter::backoff(route.clone(), delay);
                tracing::event!(
                    tracing::Level::DEBUG,
                    attempt = u64::from(attempt),
                    backoff_us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX),
                    "http.retry.backoff"
                );
                self.timer.sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RetryConfig, RetryLayer, Retryable};
    use crate::{Guarded, HttpError, Service};
    use async_lock::Semaphore;
    use bytes::Bytes;
    use http_body::{Body, Frame, SizeHint};
    use oath_adapter_net_api::{ErrorKind, Layer};
    use oath_adapter_net_mock::MockTimer;
    use std::future::Future;
    use std::num::NonZeroU32;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    // A canned one-frame response body (`Data = Bytes`, `Error = HttpError`).
    // `Debug` so `Result::unwrap_err` can render an unexpected `Ok`.
    #[derive(Debug)]
    struct StubBody {
        data: Option<Bytes>,
    }
    impl StubBody {
        fn new(body: &'static [u8]) -> Self {
            Self {
                data: Some(Bytes::from_static(body)),
            }
        }
    }
    impl Body for StubBody {
        type Data = Bytes;
        type Error = HttpError;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, HttpError>>> {
            Poll::Ready(self.get_mut().data.take().map(|d| Ok(Frame::data(d))))
        }
        fn is_end_stream(&self) -> bool {
            self.data.is_none()
        }
        fn size_hint(&self) -> SizeHint {
            self.data.as_ref().map_or_else(
                || SizeHint::with_exact(0),
                |d| SizeHint::with_exact(d.len() as u64),
            )
        }
    }

    // One scripted outcome per attempt. `Copy` so the leaf can read it by index.
    #[derive(Clone, Copy)]
    enum Step {
        Err(ErrorKind),
        Status(u16),
        StatusRetryAfter(u16, u64),
    }

    fn err_of(kind: ErrorKind) -> HttpError {
        match kind {
            ErrorKind::Timeout => HttpError::Timeout,
            ErrorKind::Connection => HttpError::connection("reset"),
            ErrorKind::Throttled => HttpError::Throttled,
            ErrorKind::Auth => HttpError::auth("expired"),
            ErrorKind::BodyTooLarge => HttpError::BodyTooLarge,
            _ => HttpError::other("boom"),
        }
    }

    // An inline leaf yielding a scripted sequence of outcomes, counting calls.
    // Once the script is exhausted it repeats the last step (so a one-element
    // `[Err(Connection)]` models an always-failing endpoint). Inline (not
    // `MockClient`) to avoid the net-http-mock -> net-http-api dev-dep cycle.
    #[derive(Clone)]
    struct ScriptLeaf {
        steps: Arc<Vec<Step>>,
        calls: Arc<AtomicUsize>,
    }
    impl ScriptLeaf {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps: Arc::new(steps),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl Service<http::Request<Bytes>> for ScriptLeaf {
        type Response = http::Response<StubBody>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            let step = self
                .steps
                .get(i)
                .copied()
                .unwrap_or_else(|| *self.steps.last().unwrap());
            async move {
                match step {
                    Step::Err(kind) => Err(err_of(kind)),
                    Step::Status(code) => {
                        let mut resp = http::Response::new(StubBody::new(b"body"));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        Ok(resp)
                    },
                    Step::StatusRetryAfter(code, secs) => {
                        let mut resp = http::Response::new(StubBody::new(b"body"));
                        *resp.status_mut() = http::StatusCode::from_u16(code).unwrap();
                        resp.headers_mut()
                            .insert(http::header::RETRY_AFTER, http::HeaderValue::from(secs));
                        Ok(resp)
                    },
                }
            }
        }
    }

    // An inline leaf whose FIRST response is a 5xx whose body holds a real
    // `Guarded` concurrency permit (max = 1); later responses release it. If
    // `Retry` did not DROP the prior response before retrying, the second
    // attempt's `acquire_arc().await` would deadlock — so a passing test proves
    // drop-before-retry.
    #[derive(Clone)]
    struct PermitLeaf {
        sem: Arc<Semaphore>,
        calls: Arc<AtomicUsize>,
    }
    impl PermitLeaf {
        fn new() -> Self {
            Self {
                sem: Arc::new(Semaphore::new(1)),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }
    impl Service<http::Request<Bytes>> for PermitLeaf {
        type Response = http::Response<Guarded<StubBody>>;
        type Error = HttpError;
        #[allow(clippy::manual_async_fn)]
        fn call(
            &self,
            _req: http::Request<Bytes>,
        ) -> impl Future<Output = Result<Self::Response, HttpError>> + Send {
            let sem = self.sem.clone();
            let i = self.calls.fetch_add(1, Ordering::Relaxed);
            async move {
                let permit = sem.acquire_arc().await; // deadlocks if the prior permit was never dropped
                if i == 0 {
                    let mut resp =
                        http::Response::new(Guarded::new(StubBody::new(b"err"), Some(permit)));
                    *resp.status_mut() = http::StatusCode::from_u16(503).unwrap();
                    Ok(resp)
                } else {
                    drop(permit); // release immediately; the success body holds nothing
                    Ok(http::Response::new(Guarded::new(
                        StubBody::new(b"ok"),
                        None,
                    )))
                }
            }
        }
    }

    fn cfg(max_attempts: u32, base: Duration, cap: Duration) -> RetryConfig {
        RetryConfig {
            max_attempts: NonZeroU32::new(max_attempts).unwrap(),
            base,
            cap,
            seed: 0x0BAD_F00D,
        }
    }

    fn req(eligible: bool) -> http::Request<Bytes> {
        let mut r = http::Request::new(Bytes::new());
        if eligible {
            r.extensions_mut().insert(Retryable);
        }
        r
    }

    // Drive a spawned retry loop to completion: yield so the task parks at each
    // backoff `sleep`, then advance past the (jittered) delay. `rounds` ≥ the
    // number of backoffs; extra advances after completion are harmless.
    async fn drain(timer: &MockTimer, rounds: u32, cap: Duration) {
        for _ in 0..rounds {
            tokio::task::yield_now().await;
            timer.advance(cap);
        }
    }

    #[tokio::test]
    async fn not_eligible_sends_once_even_on_a_transient_error() {
        // No `Retryable` marker → the fail-safe default: one send, error verbatim.
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection), Step::Status(200)]);
        let svc = RetryLayer::new(
            cfg(3, Duration::from_millis(1), Duration::from_millis(1)),
            MockTimer::new(),
        )
        .layer(leaf.clone());
        let err = svc.call(req(false)).await.unwrap_err();
        assert!(matches!(err, HttpError::Connection(_)));
        assert_eq!(leaf.calls(), 1, "not eligible → never retried");
    }

    #[tokio::test]
    async fn max_attempts_one_sends_once_even_when_eligible_and_transient() {
        // `attempt < max` is `1 < 1` = false, so no retry — and no backoff, so no
        // timer advancing is needed.
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection), Step::Status(200)]);
        let svc = RetryLayer::new(
            cfg(1, Duration::from_millis(1), Duration::from_millis(1)),
            MockTimer::new(),
        )
        .layer(leaf.clone());
        let err = svc.call(req(true)).await.unwrap_err();
        assert!(matches!(err, HttpError::Connection(_)));
        assert_eq!(leaf.calls(), 1, "max_attempts = 1 → exactly one send");
    }

    #[tokio::test]
    async fn eligible_transient_error_retries_then_succeeds() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, 3, cap).await;
        let resp = waiter
            .await
            .unwrap()
            .expect("retry succeeds on the 2nd attempt");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }

    #[tokio::test]
    async fn eligible_5xx_is_retried() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        let leaf = ScriptLeaf::new(vec![Step::Status(503), Step::Status(200)]);
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, 3, cap).await;
        let resp = waiter.await.unwrap().expect("503 retried → 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }

    #[tokio::test]
    async fn retry_after_on_a_5xx_sets_the_backoff_floor() {
        // base = 0 → jittered ceiling 0 → jittered = 0. A 503 carrying Retry-After: 5
        // must make the retry sleep 5s (the server floor), not 0. cap = 10s ≥ 5s, so
        // the honored value is not clamped. If honoring regressed, the delay would be 0
        // and the retry would fire immediately with no 5s park.
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![Step::StatusRetryAfter(503, 5), Step::Status(200)]);
        let svc = RetryLayer::new(
            cfg(3, Duration::ZERO, Duration::from_secs(10)),
            timer.clone(),
        )
        .layer(leaf.clone());
        let handle = tokio::spawn(async move { svc.call(req(true)).await });
        tokio::task::yield_now().await; // attempt 1 → 503, parks on the 5s Retry-After sleep
        assert!(
            !handle.is_finished(),
            "an honored Retry-After must park the retry; a 0 backoff would have finished it"
        );
        timer.advance(Duration::from_secs(5)); // wake the parked retry
        let resp = handle
            .await
            .unwrap()
            .expect("2nd attempt after the honored wait → 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }

    #[tokio::test]
    async fn retry_after_on_a_5xx_is_clamped_to_the_retry_cap() {
        // Retry-After: 100 but cap = 2s → the honored wait clamps to 2s, not 100s.
        let timer = MockTimer::new();
        let leaf = ScriptLeaf::new(vec![Step::StatusRetryAfter(503, 100), Step::Status(200)]);
        let svc = RetryLayer::new(
            cfg(3, Duration::ZERO, Duration::from_secs(2)),
            timer.clone(),
        )
        .layer(leaf.clone());
        let handle = tokio::spawn(async move { svc.call(req(true)).await });
        tokio::task::yield_now().await; // parks on the clamped 2s sleep
        timer.advance(Duration::from_secs(2)); // the 2s clamp elapses; a 100s sleep would not
        tokio::task::yield_now().await; // let the woken retry run attempt 2
        assert!(
            handle.is_finished(),
            "the honored value must be clamped to cap (2s), not held for 100s"
        );
        let resp = handle.await.unwrap().expect("clamped wait elapsed → 200");
        assert_eq!(resp.status(), http::StatusCode::OK);
    }

    #[tokio::test]
    async fn status_429_is_never_retried() {
        // 429 is a 4xx, not a 5xx — terminal even though eligible (ADR-0031 §2).
        let leaf = ScriptLeaf::new(vec![Step::Status(429), Step::Status(200)]);
        let svc = RetryLayer::new(
            cfg(3, Duration::from_millis(1), Duration::from_millis(1)),
            MockTimer::new(),
        )
        .layer(leaf.clone());
        let resp = svc.call(req(true)).await.expect("429 returned as Ok");
        assert_eq!(resp.status(), http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(leaf.calls(), 1, "429 never retried");
    }

    #[tokio::test]
    async fn client_4xx_is_never_retried() {
        let leaf = ScriptLeaf::new(vec![Step::Status(400)]);
        let svc = RetryLayer::new(
            cfg(3, Duration::from_millis(1), Duration::from_millis(1)),
            MockTimer::new(),
        )
        .layer(leaf.clone());
        let resp = svc.call(req(true)).await.expect("400 returned as Ok");
        assert_eq!(resp.status(), http::StatusCode::BAD_REQUEST);
        assert_eq!(leaf.calls(), 1);
    }

    #[tokio::test]
    async fn throttled_and_auth_errors_are_never_retried() {
        for kind in [ErrorKind::Throttled, ErrorKind::Auth] {
            let leaf = ScriptLeaf::new(vec![Step::Err(kind), Step::Status(200)]);
            let svc = RetryLayer::new(
                cfg(3, Duration::from_millis(1), Duration::from_millis(1)),
                MockTimer::new(),
            )
            .layer(leaf.clone());
            let err = svc.call(req(true)).await.unwrap_err();
            assert!(matches!(err, HttpError::Throttled | HttpError::Auth(_)));
            assert_eq!(leaf.calls(), 1, "{kind:?} is terminal, never retried");
        }
    }

    #[tokio::test]
    async fn attempts_exhausted_returns_the_last_outcome_verbatim() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        // Always Connection (one-element script repeats); max_attempts = 3.
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]);
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, 4, cap).await; // 2 backoffs between 3 attempts (+slack)
        let err = waiter.await.unwrap().unwrap_err();
        assert!(
            matches!(err, HttpError::Connection(_)),
            "the real error, not a synthesized one"
        );
        assert_eq!(leaf.calls(), 3, "exactly max_attempts sends");
    }

    // A request extension that counts how often the request is cloned: every
    // `Request::clone()` clones each extension, so a custom (non-derived) `Clone`
    // that bumps a shared counter observes exactly Retry's per-attempt clones (L2).
    struct CloneCounter(Arc<AtomicUsize>);
    impl Clone for CloneCounter {
        fn clone(&self) -> Self {
            self.0.fetch_add(1, Ordering::Relaxed);
            Self(Arc::clone(&self.0))
        }
    }

    fn counted(eligible: bool) -> (http::Request<Bytes>, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut r = req(eligible);
        r.extensions_mut()
            .insert(CloneCounter(Arc::clone(&counter)));
        (r, counter)
    }

    #[tokio::test]
    async fn terminal_send_moves_the_request_without_cloning() {
        // Ineligible ⇒ exactly one (terminal) send: the owned request is moved, not
        // cloned, even though `max_attempts` is 3 (L2).
        let (r, counter) = counted(false);
        let leaf = ScriptLeaf::new(vec![Step::Status(200)]);
        let svc = RetryLayer::new(
            cfg(3, Duration::from_millis(1), Duration::from_millis(1)),
            MockTimer::new(),
        )
        .layer(leaf);
        svc.call(r).await.expect("200");
        assert_eq!(
            counter.load(Ordering::Relaxed),
            0,
            "the terminal/only send must move the request, not clone it"
        );
    }

    #[tokio::test]
    async fn clones_only_for_attempts_that_may_be_followed() {
        // Eligible, always-transient, max = 3 ⇒ 3 sends, but only attempts 1 and 2
        // may be followed by a retry, so exactly 2 clones; attempt 3 moves (L2).
        let (r, counter) = counted(true);
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::Connection)]);
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(r).await });
        drain(&timer, 4, cap).await;
        waiter.await.unwrap().unwrap_err();
        assert_eq!(leaf.calls(), 3, "3 sends");
        assert_eq!(
            counter.load(Ordering::Relaxed),
            2,
            "cloned before attempts 1 and 2; the 3rd (terminal) send moves"
        );
    }

    #[test]
    fn cloned_retry_services_decorrelate_their_jitter() {
        // L3: cloning a Retry service (the ADR's per-task pattern) must give the
        // clone a divergent jitter stream — not a replay of the parent's.
        let base = RetryLayer::new(
            cfg(3, Duration::from_millis(50), Duration::from_millis(50)),
            MockTimer::new(),
        )
        .layer(ScriptLeaf::new(vec![Step::Status(200)]));
        let clone = base.clone();
        let ceil = Duration::from_millis(50);
        let differs = (0..64).any(|_| base.rng.duration_in(ceil) != clone.rng.duration_in(ceil));
        assert!(
            differs,
            "a cloned Retry service must not replay the parent's backoff sequence"
        );
    }

    // `resp`'s body (`Guarded<StubBody>`) holds a `SemaphoreGuardArc`, so clippy
    // flags it as a "significant drop" outliving its last read; the assertions
    // on `resp`/`leaf` after the `await` are exactly the point of this test, so
    // there is nothing to tighten (same rationale as `body.rs`'s guard tests).
    #[expect(
        clippy::significant_drop_tightening,
        reason = "resp's Guarded permit outliving the final assertions is the behavior under test"
    )]
    #[tokio::test]
    async fn prior_response_permit_is_released_before_the_retry() {
        let timer = MockTimer::new();
        let cap = Duration::from_millis(10);
        let leaf = PermitLeaf::new(); // 5xx holding a permit, then 200
        let svc = RetryLayer::new(cfg(3, cap, cap), timer.clone()).layer(leaf.clone());
        let waiter = tokio::spawn(async move { svc.call(req(true)).await });
        drain(&timer, 3, cap).await;
        // If `Retry` did not drop the 503 (releasing its Guarded permit) before the
        // 2nd attempt, this `await` would hang on the leaf's `acquire_arc`.
        let resp = waiter
            .await
            .unwrap()
            .expect("permit freed → 2nd attempt acquires and succeeds");
        assert_eq!(resp.status(), http::StatusCode::OK);
        assert_eq!(leaf.calls(), 2);
    }

    #[test]
    fn backoff_ceiling_clamps_and_saturates() {
        // attempt = 1 → ceiling == base (base <= cap).
        assert_eq!(
            super::backoff_ceiling(Duration::from_millis(10), Duration::from_secs(1), 1),
            Duration::from_millis(10)
        );
        // Growth then clamp: base * 2^(n-1) exceeds cap → cap (100ms * 4 = 400ms > 250ms).
        assert_eq!(
            super::backoff_ceiling(Duration::from_millis(100), Duration::from_millis(250), 3),
            Duration::from_millis(250)
        );
        // cap < base clamps to cap even at attempt 1.
        assert_eq!(
            super::backoff_ceiling(Duration::from_secs(5), Duration::from_secs(1), 1),
            Duration::from_secs(1)
        );
        // A large attempt saturates (no overflow/panic) to cap.
        assert_eq!(
            super::backoff_ceiling(Duration::from_secs(1), Duration::from_secs(30), 40),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn backoff_ceiling_doubles_each_attempt_until_the_cap() {
        // base = 10ms, cap = 10s (high enough that clamping never fires for attempts
        // 1..=4): the ceiling must be base·2^(attempt-1) — 10, 20, 40, 80 ms. Every
        // loop-driving retry test uses base == cap, so the doubling law is otherwise
        // unpinned; a 2^attempt (overshoot) or "no doubling" bug lands here.
        let base = Duration::from_millis(10);
        let cap = Duration::from_secs(10);
        assert_eq!(
            super::backoff_ceiling(base, cap, 1),
            Duration::from_millis(10)
        );
        assert_eq!(
            super::backoff_ceiling(base, cap, 2),
            Duration::from_millis(20)
        );
        assert_eq!(
            super::backoff_ceiling(base, cap, 3),
            Duration::from_millis(40)
        );
        assert_eq!(
            super::backoff_ceiling(base, cap, 4),
            Duration::from_millis(80)
        );
    }

    #[tokio::test]
    async fn body_too_large_is_not_retried() {
        // BodyTooLarge is not a transient kind, so even an eligible request sends once.
        let leaf = ScriptLeaf::new(vec![Step::Err(ErrorKind::BodyTooLarge)]);
        let svc = RetryLayer::new(
            cfg(3, Duration::from_millis(1), Duration::from_millis(1)),
            MockTimer::new(),
        )
        .layer(leaf.clone());
        let err = svc.call(req(true)).await.unwrap_err();
        assert!(matches!(err, HttpError::BodyTooLarge));
        assert_eq!(
            leaf.calls(),
            1,
            "BodyTooLarge is non-transient → never retried"
        );
    }
}

#[cfg(test)]
mod rng_tests {
    use super::SplitMix64;
    use std::time::Duration;

    #[test]
    fn same_seed_reproduces_the_same_sequence() {
        let a = SplitMix64::new(0x1234_5678);
        let b = SplitMix64::new(0x1234_5678);
        let ceil = Duration::from_secs(1);
        for _ in 0..64 {
            assert_eq!(
                a.duration_in(ceil),
                b.duration_in(ceil),
                "seeded PRNG is deterministic"
            );
        }
    }

    #[test]
    fn distinct_seeds_diverge() {
        let a = SplitMix64::new(1);
        let b = SplitMix64::new(2);
        let ceil = Duration::from_secs(1);
        // Over many draws the two sequences must differ somewhere (not lockstep).
        let differs = (0..64).any(|_| a.duration_in(ceil) != b.duration_in(ceil));
        assert!(
            differs,
            "different seeds must not produce identical sequences"
        );
    }

    #[test]
    fn draws_never_exceed_the_ceiling() {
        let rng = SplitMix64::new(42);
        let ceil = Duration::from_micros(500);
        for _ in 0..10_000 {
            assert!(
                rng.duration_in(ceil) <= ceil,
                "full jitter stays within [0, ceil]"
            );
        }
    }

    #[test]
    fn zero_ceiling_yields_zero() {
        let rng = SplitMix64::new(7);
        assert_eq!(rng.duration_in(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn saturating_ceiling_does_not_panic_and_stays_inclusive() {
        // A ceil whose nanos exceed u64::MAX clamps `ceil_nanos` to u64::MAX, hitting
        // the inclusive-edge branch (`checked_add(1)` is `None`). It must not panic
        // and every draw must stay within the clamped ceiling of u64::MAX nanos.
        let rng = SplitMix64::new(123);
        let clamped_ceil = Duration::from_nanos(u64::MAX);
        for _ in 0..1000 {
            assert!(rng.duration_in(Duration::MAX) <= clamped_ceil);
        }
    }

    #[test]
    fn clone_decorrelates_from_its_parent() {
        // L3: a cloned generator must NOT replay its parent's sequence — otherwise
        // the ADR's clone-per-task pattern synchronizes backoff across tasks.
        let a = SplitMix64::new(99);
        let b = a.clone();
        let ceil = Duration::from_millis(50);
        let differs = (0..64).any(|_| a.duration_in(ceil) != b.duration_in(ceil));
        assert!(differs, "a clone must diverge from its parent's sequence");
    }

    #[test]
    fn splitmix64_matches_the_reference_golden_vector() {
        // Absolute reference vector from https://prng.di.unimi.it/splitmix64.c for
        // seed = 0: next() finalizes (state += GOLDEN) each call. Guards against silent
        // algorithm drift — a changed STEP or finalizer constant breaks deterministic
        // backoff replay, which a same-seed determinism test can NOT catch.
        let rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
    }
}
