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
