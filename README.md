# Artemis

A decentralized, privacy-first chat application built in Rust with a hybrid P2P and relay architecture.

## Features

- **GitHub OAuth** authentication (Device Flow for desktop)
- **Real-time messaging** via WebSocket relay
- **End-to-end encrypted DMs** using X25519 + ChaCha20-Poly1305
- **P2P connections** via QUIC transport with GitHub Gist-based signaling
- **Server/channel organization** (Discord/Revolt-style)
- **Friend system** with requests, presence, and encrypted direct messages
- **Desktop GUI** built with [iced](https://iced.rs) (dark theme, Revolt-style)

## Architecture

```
crates/
  artemis-client/    # Desktop GUI (iced)
  artemis-server/    # Backend API + WebSocket relay (Axum + PostgreSQL)
  artemis-core/      # Shared models and protocol definitions
  artemis-auth/      # GitHub OAuth integration
  artemis-p2p/       # P2P networking (QUIC, crypto, signaling)
```

**Hybrid model:** Messages can be relayed through the server or sent directly peer-to-peer. DMs are always end-to-end encrypted regardless of transport.

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [PostgreSQL](https://www.postgresql.org/) 16+ (or Docker)
- A [GitHub OAuth App](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app) (for authentication)

## Quick Start

### 1. Database

Start PostgreSQL with Docker:

```bash
docker compose up -d
```

Or configure your own instance and set `DATABASE_URL`.

### 2. Environment

```bash
cp .env.example .env
# Edit .env with your GitHub OAuth credentials
```

### 3. Server

```bash
cargo run --bin artemis-server
```

### 4. Client

```bash
cargo run --bin artemis
```

## Configuration

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `postgres://artemis:artemis@localhost:5432/artemis` | PostgreSQL connection string |
| `BIND_ADDR` | `0.0.0.0:3000` | Server listen address |
| `BASE_URL` | `http://localhost:3000` | Public server URL |
| `GITHUB_CLIENT_ID` | — | GitHub OAuth App client ID |
| `GITHUB_CLIENT_SECRET` | — | GitHub OAuth App client secret |

## API

### REST

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/health` | Health check |
| `GET` | `/auth/github` | GitHub OAuth redirect |
| `GET` | `/api/v1/servers` | List user's servers |
| `POST` | `/api/v1/servers` | Create a server |
| `GET` | `/api/v1/channels/{id}/messages` | Fetch message history |
| `GET` | `/api/v1/me` | Current user profile |

### WebSocket

Connect to `/ws` with an auth token. See `crates/artemis-core/src/protocol.rs` for the full event protocol.

## Tech Stack

- **Backend:** Axum, PostgreSQL, SQLx, Tokio
- **Frontend:** iced (Rust GUI framework)
- **P2P:** QUIC (quinn), rustls, X25519-dalek, ChaCha20-Poly1305
- **Auth:** GitHub OAuth 2.0 Device Flow

## License

[MIT](LICENSE)
