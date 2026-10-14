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

Requires [Rust](https://rustup.rs/) 1.94 or newer.

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

## Code signing policy

Every release asset is signed by the Project Colony organisation in CI, never on
a developer machine. A release carries the desktop client for each platform
(`artemis-linux`, `artemis-windows.exe`, `artemis-macos`, `artemis-macos-x86`)
and the relay for Linux (`artemis-relay-linux`). Next to each asset on the
release page:

| File | What it is |
|---|---|
| `<asset>.sig` | an ed25519 signature over the asset, made with the organisation's release key |
| `<asset>.meta` | three lines binding the asset to its file name, its sha256 and the release version |
| `<asset>.meta.sig` | an ed25519 signature over the `.meta` |

The private key is an organisation secret, used only by the shared
[sign-and-publish workflow](https://github.com/Project-Colony/Project-Colony-Resources/blob/main/.github/workflows/sign-and-publish.yml)
in a job that builds nothing; the jobs that compile Artemis never see it.
Colony checks all three files before it installs or updates Artemis, and
refuses a release older than the one installed. To check a download yourself
with OpenSSL 3 (the same commands work for every asset):

```bash
cat > colony-release.pub <<'EOF'
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEARNjg3Nn8H6/aBg1unwGjkUTcrdTxERNefVaqU8cFu0s=
-----END PUBLIC KEY-----
EOF
a=artemis-linux
openssl pkeyutl -verify -pubin -inkey colony-release.pub -rawin -in "$a" -sigfile "$a.sig"
openssl pkeyutl -verify -pubin -inkey colony-release.pub -rawin -in "$a.meta" -sigfile "$a.meta.sig"
cat "$a.meta"     # version=<tag>, asset=<file name>, sha256=<digest>
sha256sum "$a"    # the digest must equal the sha256 line
```

How releases are built, signed and published:
[design/releases.md](https://github.com/Project-Colony/Project-Colony-Resources/blob/main/design/releases.md#5-signing).

## Privacy

Artemis sends no telemetry, no analytics and no crash reports. The desktop
client talks to one relay, and that relay sees everything below. For now the
client only connects to a relay on the same machine (`localhost:3000`).

What the desktop client keeps and sends:

| Data | Stored or sent | Where, and why |
|---|---|---|
| Session token | kept in memory | Issued by the relay when you sign in, and sent back to it to open each connection. Nothing is written to disk: closing Artemis signs you out. |
| Encryption key pair | kept in memory | Made at each sign-in. Its public half goes to the relay, which gives it to your friends so they can encrypt direct messages to you. |
| Messages, friend requests, server and profile changes | sent | To the relay, at `http://localhost:3000` over an unencrypted connection for now, when you send or change them. |
| GitHub sign-in | opened in your browser | `https://github.com/login/device`, where you enter the code the relay gives you. The client never holds a GitHub token. |

What the relay keeps:

| Data | Stored or sent | Where, and why |
|---|---|---|
| GitHub account | stored | Your GitHub user ID, login, name and avatar URL. The relay asks GitHub for the `read:user` scope only, reads your profile from `api.github.com` once at sign-in, and drops the GitHub token without storing or logging it. |
| Sessions | stored | The SHA-256 of each session token, never the token itself, with when it was created and last used. A session expires 30 days after it was last used. |
| Public key | stored | The public half of your key pair, given to your friends. |
| Servers and channels | stored | The servers, categories and channels you create or join, and your role in each. |
| Custom emojis | stored | For each server, the name and image URL of every custom emoji, and who added it. |
| Server messages | stored, readable by the relay | The text of every message in a server channel, with its author, time, replies, reactions, pins and mentions, and how far you have read each channel. Server messages are not end-to-end encrypted. |
| Direct messages | stored, encrypted | The ciphertext, its sender, its recipient and when it was sent. Only you and your friend can decrypt it, provided the public keys are genuine: the relay hands them out and Artemis does not verify them yet, so whoever runs the relay could substitute a key and read the messages that follow. |
| Friends | stored | Friend requests and friendships. |
| Status | stored | Whether you are online, and your custom status. |
| Logs | written by the relay | Relay and GitHub user IDs when users sign in, add friends, publish keys or hit errors; never message text. |

The client connects to no server except the relay, and opens GitHub's sign-in
page only when you sign in. The relay connects to no server except GitHub
(`github.com` and `api.github.com`), and only to sign users in. GitHub's own
handling of your account is in the
[GitHub General Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).

## License

Artemis is free software, licensed under the
[GNU General Public License v3.0 or later](LICENSE).

The bundled JetBrains Mono Nerd Font files in `crates/artemis-client/assets/fonts/`
are distributed under the SIL Open Font License 1.1.
