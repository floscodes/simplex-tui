# Native Rust client port

The target is to run `simplex-tui` without GHC or `libsimplex`, while preserving
interoperability with the pinned SimpleX Chat release and existing user data.
The current application does **not** meet that target. `libsimplex-rs` parses
the controller's JSON in Rust, but `Simplex.Chat.Mobile` still owns the client.

## Current boundary

- `crates/libsimplex-rs/src/ffi.rs`: GHC runtime and `chat_migrate_init`,
  `chat_send_cmd`, `chat_recv_msg_wait` ABI calls.
- `crates/libsimplex-rs/src/client.rs`: Rust command/event worker using that ABI.
- `crates/libsimplex-rs/src/model.rs`: JSON-to-Rust model translation.
- `crates/libsimplex-rs/src/protocol.rs`: native inline file chunk codec and
  chat version selection.
- `crates/libsimplex-rs/src/transport.rs`: unencrypted SMP block framing,
  client version selection, and server hello/channel-binding parsing, including
  the raw certificate chain layout and SHA-256 identity binding to a
  TLS-validated peer chain.
- `crates/libsimplex-rs/src/auth.rs`: Ed25519/Ed448 verification of the
  server's signed X25519/X448 key against the TLS leaf certificate.
- `crates/libsimplex-rs/src/tls.rs`: TLS stream with pinned identity and
  certificate-chain validation, ALPN, and TLS channel binding.
- `crates/libsimplex-rs/src/handshake.rs`: authenticated initial SMP handshake,
  ephemeral X25519/X448 key exchange, and versioned client hello.
- `crates/libsimplex-rs/src/encrypted.rs`: SMP v11+ XSalsa20-Poly1305 blocks
  with HKDF-SHA-512 send/receive chains. SMP commands, subscriptions, and
  agent state are not ported.
- `vendor/simplex-chat/src/Simplex/Chat/Mobile.hs`: ABI entry point.
- `vendor/simplex-chat/src/Simplex/Chat/Controller.hs` and `Library/`: chat
  behavior and command handling.
- `vendor/simplex-chat/src/Simplex/Chat/Store/`: chat database and migrations.
- `simplexmq` (pinned in `vendor/simplex-chat/cabal.project`): messaging agent,
  SMP/XFTP transport, cryptography, and agent store.

## Migration sequence

1. Port pure wire formats, version negotiation, and protocol types. Verify
   byte-for-byte vectors with the pinned Haskell implementation, including
   malformed input and version boundaries.
2. Implement a Rust messaging agent: SMP connections and subscriptions,
   XFTP transfers, queue state, retries, and the cryptographic ratchets.
   Test cross-client send/receive and key rotation against the upstream app.
3. Implement compatible chat and agent storage, including migrations from
   existing `~/.simplex-tui` databases. Test migration copies before enabling
   any native writes to an existing database.
4. Port chat state and commands used by `simplex-tui`: profiles, contacts,
   invitations, groups, messages, reactions, files, preferences, and servers.
   Compare command results and asynchronous events with the current backend.
5. Select the Rust backend only after the same TUI scenarios pass with both
   implementations. Then remove the GHC bootstrap, dynamic library packaging,
   and Haskell FFI.

The Haskell backend remains the default until these acceptance checks pass.
Passing Rust unit tests for an isolated codec does not imply a working native
client.
