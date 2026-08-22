# Pulsar AI — local-first NGX trading assistant (Tauri + LangChain)

## Structure

```
apps/desktop     Tauri 2 + Vite React UI
packages/agent   LangChain stdio worker (BYOK LLM)
packages/shared  Shared Zod types / contracts
```

## Prerequisites

- Node.js 20+
- Rust stable (`rustup`) — **required for `npm run desktop:build*`**
- macOS: Xcode Command Line Tools (`xcode-select --install`)

If `tauri build` fails with `cargo metadata ... No such file or directory`, Rust is not installed or not on PATH. Install with:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
cargo --version
```

Then run the build again from a **new terminal** (or `source ~/.cargo/env` in the current one).

## Quick start

```bash
npm install
npm run agent:build
```

**UI only (no Xcode CLT)** — browser preview with local mock data:

```bash
npm run desktop:ui
# → http://localhost:1420
```

**Full desktop app** (requires Xcode CLT):

```bash
npm run desktop:dev
```

## Configuration

Copy `.env.example` for optional agent/dev overrides. In the desktop app, NGX Pulse and LLM credentials are entered in Settings and stored in the OS keychain (not `.env`).

See [apps/desktop/README.md](apps/desktop/README.md).

## Security notes

- NGX Pulse Supabase **anon** keys are public client credentials. Never put a Supabase **service-role** key in `.env`, `tauri.conf.json`, or compiled config.
- Browser mock mode (`npm run desktop:ui`) does not persist credentials.
- Release signing, SBOM, and updater verification: [docs/release-security.md](docs/release-security.md).
