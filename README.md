# Artemis

**A chat application in Rust: an iced desktop client and an Axum + PostgreSQL relay server.**

Artemis aims to be a Discord/Revolt-style chat with servers, channels, friends and
end-to-end encrypted direct messages, signed in with a GitHub account.

## Status

Early prototype, not ready for use. What works and what does not:

- The desktop client runs and has a demo mode with sample servers, channels and messages.
- Signing in from the desktop client does not reach a working session yet. The client
  sends its GitHub token to the relay, but the relay only accepts the tokens it issues
  through its own web OAuth callback.
- The relay URL is fixed to `http://localhost:3000` (plain WebSocket, no TLS).
- A new encryption key pair is generated at every sign-in, so older direct messages
  cannot be decrypted after a restart.
- A direct message is sent unencrypted when the friend has not published a public key.
- Friends' public keys come from the relay and are not verified (no fingerprint check or
  key pinning yet), so whoever runs the relay can substitute a key and read or alter
  direct messages. Encryption keeps the text unreadable in the relay's database and on
  the network, but does not protect it from the relay operator.
- The `artemis-p2p` crate (QUIC transport, GitHub Gist signaling) is not wired into the
  client. Only its key exchange and encryption code is used.

## Layout

```
crates/
  artemis-client/    desktop client (iced), binary "artemis"
  artemis-server/    relay server (Axum + PostgreSQL), binary "artemis-server"
  artemis-core/      shared models and wire protocol
  artemis-auth/      GitHub OAuth
  artemis-p2p/       P2P networking and encryption (mostly unused)
```

## Build from source

Requires a stable [Rust](https://rustup.rs/) toolchain.

```bash
cargo build --release --workspace
```

The client runs on its own:

```bash
cargo run --bin artemis
```

The relay server needs PostgreSQL 16 or later and a GitHub OAuth app. It reads its
settings from the environment and from a `.env` file (see `.env.example`):

1. Copy `.env.example` to `.env`.
2. Choose a database password of letters and digits, and set it as `POSTGRES_PASSWORD`
   and in `DATABASE_URL`. Other characters need quoting in `.env` and percent-encoding
   in the URL.
3. Create an OAuth app at <https://github.com/settings/developers> with the callback
   URL `http://localhost:3000/auth/github/callback`, then set `GITHUB_CLIENT_ID` and
   `GITHUB_CLIENT_SECRET`.
4. Start the database, which listens on `127.0.0.1:5432` only, then the relay:

```bash
docker compose up -d
cargo run --bin artemis-server
```

The relay refuses to start while `DATABASE_URL`, `GITHUB_CLIENT_ID` or
`GITHUB_CLIENT_SECRET` is missing. It listens on `127.0.0.1:3000` unless `BIND_ADDR`
says otherwise, and logs at `info` unless `RUST_LOG` says otherwise.

## License

Artemis is free software, licensed under the
[GNU General Public License v3.0 or later](LICENSE).

The bundled JetBrains Mono Nerd Font files in `crates/artemis-client/assets/fonts/`
are distributed under the SIL Open Font License 1.1.
