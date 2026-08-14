# Release signing, updates, and supply chain

Pulsar AI desktop builds must be treated as production trading software.

## Code signing and notarization

- **macOS:** Sign the `.app` with a Developer ID Application certificate and notarize with Apple (`notarytool`). Gatekeeper must accept the bundle before distribution.
- **Windows:** Sign `Pulsar AI.exe` and the installer with an Authenticode certificate.
- Store certificates in the CI/CD secret store; never commit `.p12` / `.pfx` files.

## Signed updates

- Publish Tauri updater artifacts with a signed `latest.json` manifest.
- The app must verify the update signature before applying it.
- Keep a rollback channel (previous signed build) for emergency patches.

## SBOM and provenance

- Generate an SBOM (`cyclonedx` or `syft`) for each tagged release.
- Record npm and Cargo lockfile hashes in the release notes.
- Prefer GitHub Artifact Attestations / SLSA provenance for release binaries.

## Secrets and Supabase

- The Pulse **anon** key is a public client credential. It is not a secret and must never be confused with a Supabase **service-role** key.
- Never compile or bundle a service-role key. External release gate: confirm RLS/policies before shipping a build that embeds anon config.
- User Pulse, LLM, and Wealth credentials stay in the OS keychain.

## Known dependency advisories

`npm audit --omit=dev` may still report LangSmith (transitive via `@langchain/core` 0.3) and Lodash (via Recharts). Do not `npm audit fix --force` onto LangChain 1.x without a dedicated worker migration. Track upgrades in the next dependency sprint.

## Emergency patch

1. Cut a signed patch build from `main` (or a hotfix branch).
2. Ship a signed updater manifest that points only at the patched artifact.
3. Revoke live trading remotely only by instructing users to disable **Enable live trading** (backend setting is local; there is no cloud kill-switch).
