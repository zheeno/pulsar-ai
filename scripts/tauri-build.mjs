/**
 * Run `tauri build` with OS-specific bundles.
 * Usage: node scripts/tauri-build.mjs [mac|win|current]
 */
import { spawn } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const desktopDir = join(root, "apps/desktop");
const target = (process.argv[2] || "current").toLowerCase();

const bundlesByTarget = {
  mac: "app,dmg",
  darwin: "app,dmg",
  win: "nsis",
  win32: "nsis",
  windows: "nsis",
};

function resolveBundles() {
  if (target === "current") {
    return bundlesByTarget[process.platform];
  }
  return bundlesByTarget[target];
}

const bundles = resolveBundles();
if (!bundles) {
  console.error(
    `Unknown build target "${target}". Use mac, win, or omit for the current OS.`,
  );
  process.exit(1);
}

if (target === "mac" && process.platform !== "darwin") {
  console.error("macOS bundles must be built on a Mac.");
  process.exit(1);
}
if (target === "win" && process.platform !== "win32") {
  console.error("Windows installers must be built on Windows.");
  process.exit(1);
}

const args = ["tauri", "build", "--bundles", bundles];
console.log(`[tauri-build] ${args.join(" ")}`);

const child = spawn("npx", args, {
  cwd: desktopDir,
  env: process.env,
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
