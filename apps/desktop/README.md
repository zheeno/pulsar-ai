# Pulsar AI Desktop

Local-first Tauri app for NGX sandbox trading with BYOK credentials.

## Layout

- `src/` — Vite + React UI
- `src-tauri/` — Rust core (SQLite, NGX Pulse, execution, scheduler)
- `packages/agent` — LangChain worker (spawned by Tauri)

## Run

```bash
# From repo root
npm install
npm run agent:build

# Browser UI with mock data (no Xcode CLT)
npm run desktop:ui

# Full Tauri window (needs Xcode Command Line Tools)
npm run desktop:dev
```

## Secrets

Entered in Settings / first-run wizard; stored in OS keychain (`com.pulsar.ai`). SQLite lives under the app data directory.
