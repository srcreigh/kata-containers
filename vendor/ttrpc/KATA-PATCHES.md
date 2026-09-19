# kata-fc transport hardening

Base: crates.io ttrpc 0.9.0, upstream f31f5925749bba2616bc53942dd83877a3f5b532.
The original Apache-2.0 license and source notices are retained. This is the
single locked transport, not an optional backend. Track upstream security fixes.

Changes: async unary registration cleanup on cancellation/timeouts, bounded client
registrations and queues, inline nonblocking response dispatch, unary frame-state
validation, immediate rejection of oversized frames, and a 30-second deadline
starting with the first byte of each frame (idle connections remain valid).
Transport tests cover cancellation, timeouts, malformed/oversized responses and
continued ordinary RPC operation. Upstream example fixtures are not vendored.

The supported target is x86_64 Linux. Both async and sync APIs remain: the
containerd shim protocol dependency requires sync, while the runtime and agent
use async; the sync-only crossbeam dependency is optional with that feature. Transports are Unix sockets (filesystem and abstract) and vsock.
Removed TCP address/connection/listener implementations and its socket options,
Windows named pipes/dependencies, and macOS/Android compatibility branches.
Unsupported transport schemes fail before socket creation. Rejection tests cover
TCP (IPv4/IPv6), Windows named pipes and unknown schemes. The frame hardening and
its existing regression tests are unchanged.

Removed the obsolete async recoverable-frame-error dispatch and its private
client/server handlers. The hardened frame reader already reports every framing
failure as a fatal `InternalError`; it never produces the removed `ReturnError`
variant. Generic request/status errors and unary/stream handling are unchanged.
The `GenMessageError` type and frame-reader return signature remain; the locked
containerd 0.11 consumers use neither the removed variant nor these private
handlers.

Removed redundant sync successful-write length checks. Its private write helper
already loops over short writes until the complete header/body is written or a
socket error occurs; it now returns `Result<()>` and derives the target length
from the supplied slice. Read-side EOF/length checks and the sync framing-error
contract are unchanged.
