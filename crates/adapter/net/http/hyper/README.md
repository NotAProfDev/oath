# oath-adapter-net-http-hyper

The hyper backend for the OATH net-http stack: `build()` assembles the canonical
resilience stack (rate-limit → retry → circuit-breaker → timeout → tracing) over a
pooled `hyper_util` client with a rustls HTTPS connector.

## The per-request extension protocol

Every request carries its resilience directives as `http::Extensions`. Stamp them
with `req.extensions_mut().insert(..)` **before** calling the client:

| Directive | Required? | Absent default | Purpose |
|---|---|---|---|
| `RateScope<K>` | **Yes** (fail-closed) | rejected as `Throttled` — never sent | Which pacing bucket(s) to spend: `None` / `Global` / `Local(k)` / `Both(k)` |
| `Retryable` | No | request sent once | Opt this request into the Retry layer (transient errors + 5xx) |
| `BufferMode` | No | `Stream` | `Buffer` collects the whole body inside the retry/breaker boundary |

> **Why fail-closed?** A missing `RateScope` is a bug, not "no limit" — silently
> unthrottled traffic can breach a venue's rate limits and trip a self-inflicted
> outage. Use `RateScope::None` to *explicitly* opt out of pacing.

See `examples/client_with_directives.rs` for a runnable end-to-end example.
