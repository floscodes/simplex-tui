# libsimplex-rs

`libsimplex-rs` is a Rust wrapper for client functionality of the SimpleX Chat protocol.

## Rust port status

The inline file chunk wire format and chat version selection from the pinned
upstream sources are implemented in `protocol`. This module can be used by a
future Rust transport without loading the Haskell runtime. The current TUI
still uses the official Haskell controller through
the C ABI for transport, cryptography, database access, and chat state.
The `transport` module also implements plaintext SMP block framing, client
version selection, and the server hello prefix from the pinned `simplexmq`
source. It also parses the unverified certificate chain bytes. The `tls` module
establishes a TLS stream after validating the pinned identity and certificate
chain. The `auth` and `handshake` modules verify the signed server key, exchange
an ephemeral DH key, and send the versioned client hello. Encrypted transport
blocks are implemented in `encrypted`. The messaging agent is still missing.

A complete native client requires compatible implementations of those layers
and interoperability tests against the pinned upstream release before the ABI
can be removed.

## License

Licensed under the [GNU Affero General Public License v3.0 only](https://www.gnu.org/licenses/agpl-3.0.html).
