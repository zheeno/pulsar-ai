import { spawn } from "node:child_process";
import { assertCargo, withCargoPath } from "./cargo-env.mjs";

const args = process.argv.slice(2);
if (args.length === 0) {
  console.error("Usage: node scripts/run-with-cargo.mjs <command> [args...]");
  process.exit(1);
}

const env = withCargoPath({
  ...process.env,
  RUST_LOG: process.env.RUST_LOG ?? "info,ngx_pulse=debug",
});
assertCargo(env);

const [command, ...commandArgs] = args;
const child = spawn(command, commandArgs, {
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
