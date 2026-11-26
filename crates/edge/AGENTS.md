# oss-edge: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- The front holds no handler logic: a stream is translated to one HTTP/1.1 request on a loopback connection to the node's own listener, so `oss-rest`'s limits and error shapes answer it ([ADR 0058](../../docs/adr/0058-modern-protocols-front.md)).
- Every buffer is charged to the node budget when the front starts (`EdgeConfig::charge`); a stream or connection past its bound is refused, never queued without limit.
- Flow-control credit goes back to the client only after the loopback socket accepted the bytes.
- The front never sets `X-Forwarded-For` and no handler treats a loopback peer as privileged.
- Only this crate (and the node's `edge` feature) may depend on tokio, h2, quinn, h3 or rustls.
