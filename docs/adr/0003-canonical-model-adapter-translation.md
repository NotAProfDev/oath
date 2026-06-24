# Canonical core model with adapter-side translation

The Core and the Bus speak exactly one canonical vocabulary (`Symbol`, `Price`,
`Quantity`, …), chosen at compile time and identical for every adapter. Each
**Adapter** owns the translation between its venue's representation and the
canonical model at its own boundary (an anti-corruption layer); venue-specific
representations never leak inward. A central symbology (e.g. perm_id / OpenFIGI)
gives the canonical identity, so the same instrument offered by different
brokers collapses to a single `Symbol` and is not double-counted by portfolio or
risk.

## Consequences

- Every adapter carries a translation layer (precision conversion, symbol
  resolution, mapping tables) — accepted cost, paid to keep the core clean.
- `Price`/`Quantity` are newtypes over a swappable inner numeric type
  (`rust_decimal` for the MVP — 16-byte, `Copy`, heap-free, so zero-copy-safe);
  the inner type can later become fixed-point `i64` or a bignum at compile time
  without touching call sites.
- Translation lives in the adapter process, so a malformed-message bug in one
  venue's translation cannot corrupt another's.
