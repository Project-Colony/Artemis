# Artemis

**A chat application in Rust: an iced desktop client and an Axum + PostgreSQL relay server.**

Artemis aims to be a Discord/Revolt-style chat with servers, channels, friends and
end-to-end encrypted direct messages, signed in with a GitHub account.

## Status

Early prototype, not ready for use. What works and what does not:

- The desktop client runs and has a demo mode with sample servers, channels and messages.
- Signing in from the desktop client does not reach a working session yet. The relay
  signs users in through GitHub's device flow and issues its own session tokens, but
  the client still runs the device flow against GitHub itself and sends the GitHub
  token, which the relay refuses.
- A session cannot be signed out or revoked yet, not even by revoking the app on
  GitHub, and a connected socket stays signed in after its session has expired.
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
  artemis-auth/      GitHub device flow sign-in
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
3. Create an OAuth app at <https://github.com/settings/developers>, check
   "Enable Device Flow", and set its client ID as `GITHUB_CLIENT_ID`. The relay needs
   no client secret, and does not use the app's callback URL.
4. Start the database, which listens on `127.0.0.1:5432` only, then the relay:

```bash
docker compose up -d
cargo run --bin artemis-server
```

The relay refuses to start while `DATABASE_URL` or `GITHUB_CLIENT_ID` is missing. It
listens on `127.0.0.1:3000` unless `BIND_ADDR` says otherwise, and logs at `info` unless
`RUST_LOG` says otherwise.

### Signing in

A client signs in to the relay in three steps:

1. `POST /auth/device/start` returns `device_code`, `user_code`, `verification_uri`,
   `interval` and `expires_in`. The user opens `https://github.com/login/device` and
   enters `user_code`. A client opens that address itself rather than the
   `verification_uri` it was sent, so a relay cannot send its users to another page.
2. Every `interval` seconds, `POST /auth/device/poll` with `{"device_code": "..."}`.
   The answer's `status` is `pending` (poll again), `slow_down` (poll every `interval`
   seconds from now on), `denied`, `expired`, or `complete` with a `token`, the
   `username` and the `avatar_url`. A device code that is empty, over 64 characters or
   not printable ASCII gets 400, GitHub failures 502 and database failures 500, all
   with an `error`.
3. Send the token in `Authenticate` on `/ws`.

The relay asks GitHub for the `read:user` scope only. It uses the GitHub token once to
read the user's profile and drops it, never storing or logging it. A session token is
32 random bytes, of which the relay stores only the SHA-256. A session expires 30 days
after it last signed a socket in, however long that socket stays connected.

Usernames are GitHub logins, unique whatever their case, as on GitHub. When someone
takes a login that an account on the relay still holds from before a rename on GitHub,
that account is renamed `#` followed by its GitHub user ID until its user signs in
again.

## License

Artemis is free software, licensed under the
[GNU General Public License v3.0 or later](LICENSE).

The bundled JetBrains Mono Nerd Font files in `crates/artemis-client/assets/fonts/`
are distributed under the SIL Open Font License 1.1.
