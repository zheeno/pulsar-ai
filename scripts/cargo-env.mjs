import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";

const pathSep = process.platform === "win32" ? ";" : ":";

/** Common locations for rustup-installed cargo (macOS GUI apps often miss shell PATH). */
export function cargoBinDirs() {
  const dirs = [join(homedir(), ".cargo", "bin")];
  if (process.platform === "darwin") {
    dirs.push("/opt/homebrew/bin", "/usr/local/bin");
  }
  return dirs;
}

export function withCargoPath(env = process.env) {
  const prefix = cargoBinDirs().join(pathSep);
  const current = env.PATH ?? "";
  const already = cargoBinDirs().some((dir) => current.split(pathSep).includes(dir));
  return {
    ...env,
    PATH: already ? current : `${prefix}${pathSep}${current}`,
  };
}

export function resolveCargoBin(env = withCargoPath()) {
  for (const dir of cargoBinDirs()) {
    const candidate = join(dir, process.platform === "win32" ? "cargo.exe" : "cargo");
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  try {
    const cmd = process.platform === "win32" ? "where.exe" : "which";
    const arg = process.platform === "win32" ? "cargo.exe" : "cargo";
    const out = execFileSync(cmd, [arg], { encoding: "utf8", env }).trim();
    const line = out.split(/\r?\n/)[0]?.trim();
    if (line && existsSync(line)) {
      return line;
    }
  } catch {
    // fall through
  }
  return null;
}

export function assertCargo(env = withCargoPath()) {
  const cargo = resolveCargoBin(env);
  if (cargo) {
    return cargo;
  }
  console.error(
    [
      "",
      "Rust/Cargo was not found on PATH.",
      "",
      "Install Rust (includes cargo), then restart the terminal:",
      "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
      "  source \"$HOME/.cargo/env\"",
      "  rustc --version",
      "",
      "macOS builds also need Xcode Command Line Tools:",
      "  xcode-select --install",
      "",
      "After installing, run this build command again from the same terminal.",
      "",
    ].join("\n"),
  );
  process.exit(1);
}
