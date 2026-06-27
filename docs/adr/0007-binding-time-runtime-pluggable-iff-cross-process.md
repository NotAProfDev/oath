# Binding time: runtime-pluggable iff cross-process

Every OATH binary — Core, each Adapter, each Strategy Node — is fully
monomorphized internally: **no `dyn` trait object sits on any decision hot
path.** "Swappable" is achieved two different ways depending on *what* is
swapped — **compile-time generics** for anything in-process, and **process-level
pluggability over the Bus** (spawn a process, run a registration handshake) for
anything that joins at runtime. The rule: *runtime-pluggable ⟺ a separate
process across the Bus; in-process ⟹ compile-time static.*

## Considered options

- *Trait objects (`dyn`) for swappable subsystems* — rejected: dynamic dispatch
  on the per-event decision path defeats the latency goal (no inlining across the
  call, vtable indirection) and is unnecessary — the only thing that must change
  at runtime is *which processes are connected*, not which code a hot loop calls.
- *Compile-time generics in-process + process/Bus pluggability across* — chosen.

## Consequences

- Risk/execution Policies and I/O backends (net, persistence, Bus impl) are bound
  at build time via generics; a user *builds their Core* with the rules and
  backends they need. There is no runtime in-process plugin loader.
- The polymorphism that lets Core talk to "any adapter" is carried by the
  canonical message model + Bus trait (ADR-0002/0003), **not** by an in-process
  trait. Core holds a Bus handle, never a `Box<dyn Adapter>`, and never depends on
  a `Broker` / `DataProvider` / `Strategy` trait — those traits live on the
  adapter/strategy side and are called statically by their host harness.
- Adapters and Strategy Nodes are added at runtime by spawning a process that
  connects to the Bus and completes a registration handshake (ADR-0001) —
  pluggability without any in-process dynamic dispatch.
- Co-location is a *backend choice, not a code path*: an initially co-located
  Strategy (ADR-0001) talks through an in-memory Bus backend; moving it to its own
  process swaps the backend, not the strategy code. The strategy↔Core seam never
  branches on local-vs-remote.
- Cost accepted: no pre-built generic binary with runtime-loadable in-process
  plugins; each deployment is compiled for its chosen Policies/backends.
