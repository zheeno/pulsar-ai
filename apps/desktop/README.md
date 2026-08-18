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

## Ship (macOS)

Requires Node.js 20+ on the **build and run** machine (the LLM agent worker is a bundled Node script).

1. Put public Pulse config in the repo-root `.env` (`NGX_PULSE_*`).
2. Build:

```bash
npm run desktop:build
```

Artifacts:

- `apps/desktop/src-tauri/target/release/bundle/macos/Pulsar AI.app`
- `apps/desktop/src-tauri/target/release/bundle/dmg/Pulsar AI_1.0.0_*.dmg`

The DMG is **unsigned**. Recipients may need right-click → Open the first time, or you can sign/notarize with an Apple Developer ID later.

## Secrets

Entered in Settings / first-run wizard; stored in OS keychain (`com.pulsar.ai`). SQLite lives under the app data directory.
