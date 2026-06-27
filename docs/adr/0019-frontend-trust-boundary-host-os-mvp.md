# Frontend trust boundary: host-OS for MVP, auth deferred to the networked Bus

For MVP the Frontend's security boundary is the **host OS**: the Bus transport's
local access controls (shared-memory segment / socket file permissions) are the
trust boundary — any process the operator can run on the single host is trusted,
and **no application-level authentication or authorization is built**. On a
single-operator host this is the *real* boundary: anyone who can reach the
shared-memory segment can read process memory directly, so app-level auth over
localhost would be theater. The seams are **reserved, not built** — an
identity/principal field is kept on the control req/reply (ADR-0016) so
authorization can be added without a wire-format break — and control is **already
auditable**: an emergency halt is a logged Event Log input (ADR-0017) and
lifecycle commands go through the Supervisor. Authentication (tokens / mTLS) and
authorization tiering (observe-only vs control vs admin) land **together with the
networked-Bus capability** — the point at which the threat model changes from "my
own host" to "the network," and at which a networked Bus backend brings its own
transport security anyway.

## Considered options

- _App-level auth from day one_ (shared secret / token on control) — rejected for
  MVP: theater over localhost shared-memory, where the OS already gates access and
  an attacker with segment access has memory access. Its one real benefit —
  exercising the auth seam early — is bought instead by reserving the principal
  field, without paying for a mechanism that guards nothing yet.
- _No reserved seam at all_ — rejected: authorization must be addable without
  breaking the control wire format, so the principal field is reserved now even
  though unused.

## Consequences

- **The single-trusted-operator-host assumption is explicit** and must be
  revisited the instant the Bus is networked or the host becomes multi-user.
- **Auditability is independent of authentication:** halt and lifecycle are
  recorded (Event Log / Supervisor) whether or not a principal is yet
  authenticated — so "what happened" is always answerable; "who" arrives with
  authn.
- **Adjacent, but not security:** a confirmation guard on destructive ops
  (`halt Live` → typed confirm) is a CLI-UX safety feature; encryption of
  sensitive Business State is moot on localhost and arrives with the networked Bus.
