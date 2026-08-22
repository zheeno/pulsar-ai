# Pulsar AI Desktop

Local-first Tauri app for NGX sandbox trading with BYOK credentials.

## Layout

- `src/` — Vite + React UI
- `src-tauri/` — Rust core (SQLite, NGX Pulse, execution, scheduler)
- `packages/agent` — LangChain worker (spawned by Tauri via bundled Node.js)

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

`npm run desktop:dev` runs `prepare:resources`, which bundles the agent worker and (on macOS/Windows) downloads Node.js 20 into `src-tauri/resources/node/` once per version. Set `PULSAR_SKIP_NODE_BUNDLE=1` to skip the Node download during local dev if you already have Node 20+ on your PATH.

## Ship (macOS)

Requires Node.js 20+ on the **build machine** only. End users do **not** need Node installed — the release `.app` includes a bundled Node runtime for the AI agent.

### Which build to use

| Recipient Mac | Build command | App arch | Bundled Node | Minimum macOS |
|---------------|---------------|----------|--------------|---------------|
| Intel (Catalina / Big Sur / …) | `npm run desktop:build:mac:intel` | x86_64 | x64 | 10.15 |
| Apple Silicon (M1/M2/M3…) | `npm run desktop:build:mac:arm` | arm64 | arm64 | 11.0 |
| Same as your build Mac | `npm run desktop:build:mac` | native | matching | 10.15 (Intel) or 11.0 (ARM) |

**Important:** An arm64 `.app` will not open on Intel Macs (and vice versa). If a DMG says `_x64` but was built on Apple Silicon without `--target x86_64-apple-darwin`, it may still be arm64 inside — always use `desktop:build:mac:intel` for Intel users.

On an Apple Silicon Mac, install the cross target once:

```bash
rustup target add x86_64-apple-darwin
npm run desktop:build:mac:intel
```

1. Put public Pulse config in the repo-root `.env` (`NGX_PULSE_*`).
2. Build with the command from the table above.

Artifacts:

- `apps/desktop/src-tauri/target/<triple>/release/bundle/macos/Pulsar AI.app`
- `apps/desktop/src-tauri/target/<triple>/release/bundle/dmg/Pulsar AI_1.0.0_*.dmg`

The DMG is **unsigned**. On macOS 11+, recipients may need **Right-click → Open** the first time, or allow in **System Preferences → Security & Privacy**. Gatekeeper messages like “cannot be opened” are normal for unsigned builds — that is not the same as “requires a newer version of macOS”.

## Secrets

Entered in Settings / first-run wizard; stored in OS keychain (`com.pulsar.ai`). SQLite lives under the app data directory.

## Overrides

| Variable | Purpose |
|----------|---------|
| `NGX_NODE_BIN` | Force a specific Node binary (dev/debug) |
| `PULSAR_SKIP_NODE_BUNDLE=1` | Skip downloading Node during `prepare:resources` |
| `PULSAR_NODE_VERSION` | Pin bundled Node version (default `20.19.2`) |
| `PULSAR_NODE_ARCH` | `x64` or `arm64` when bundling Node for macOS |
| `CARGO_BUILD_TARGET` | Set by `tauri-build.mjs` (`x86_64-apple-darwin` / `aarch64-apple-darwin`) |
