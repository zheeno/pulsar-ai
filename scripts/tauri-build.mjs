/**
 * Run `tauri build` with OS-specific bundles.
 *
 * Usage:
 *   node scripts/tauri-build.mjs [mac|mac-intel|mac-arm|win|current]
 *
 * mac-intel  — x86_64 app + x64 Node (Intel Mac 10.15 / 11.x)
 * mac-arm    — arm64 app + arm64 Node (Apple Silicon 11+)
 * mac        — native host architecture
 */
import { spawn } from "node:child_process";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const desktopDir = join(root, "apps/desktop");
const target = (process.argv[2] || "current").toLowerCase();

const bundlesByTarget = {
  mac: "app,dmg",
  "mac-intel": "app,dmg",
  "mac-x64": "app,dmg",
  "mac-arm": "app,dmg",
  "mac-arm64": "app,dmg",
  darwin: "app,dmg",
  win: "nsis",
  win32: "nsis",
  windows: "nsis",
};

function hostArch() {
  if (process.platform === "darwin") {
    try {
      return execFileSync("uname", ["-m"], { encoding: "utf8" }).trim() === "arm64"
        ? "arm64"
        : "x64";
    } catch {
      // fall through
    }
  }
  return process.arch === "arm64" ? "arm64" : "x64";
}

function resolveMacCargoTarget() {
  if (target === "mac-intel" || target === "mac-x64") {
    return "x86_64-apple-darwin";
  }
  if (target === "mac-arm" || target === "mac-arm64") {
    return "aarch64-apple-darwin";
  }
  if (target === "mac" || target === "darwin") {
    return hostArch() === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
  }
  return null;
}

function resolveBundles() {
  if (target === "current") {
    return bundlesByTarget[process.platform];
  }
  return bundlesByTarget[target];
}

const bundles = resolveBundles();
if (!bundles) {
  console.error(
    `Unknown build target "${target}". Use mac, mac-intel, mac-arm, win, or omit for the current OS.`,
  );
  process.exit(1);
}

if ((target.startsWith("mac") || target === "darwin") && process.platform !== "darwin") {
  console.error("macOS bundles must be built on a Mac.");
  process.exit(1);
}
if (target.startsWith("win") && process.platform !== "win32") {
  console.error("Windows installers must be built on Windows.");
  process.exit(1);
}

const macCargoTarget = resolveMacCargoTarget();
const env = { ...process.env };
const args = ["tauri", "build", "--bundles", bundles];

if (macCargoTarget) {
  env.CARGO_BUILD_TARGET = macCargoTarget;
  env.PULSAR_NODE_ARCH = macCargoTarget.startsWith("x86_64") ? "x64" : "arm64";
  env.MACOSX_DEPLOYMENT_TARGET = macCargoTarget.startsWith("x86_64") ? "10.15" : "11.0";
  args.push("--target", macCargoTarget);
  console.log(
    `[tauri-build] macOS target=${macCargoTarget} deployment=${env.MACOSX_DEPLOYMENT_TARGET} node=${env.PULSAR_NODE_ARCH}`,
  );
}

console.log(`[tauri-build] ${args.join(" ")}`);

const child = spawn("npx", args, {
  cwd: desktopDir,
  env,
  stdio: "inherit",
  shell: true,
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
