// SPDX-License-Identifier: Apache-2.0
import { execFileSync, spawnSync } from "node:child_process";
import path from "node:path";

function resolveSharedTarget() {
  const commonDir = execFileSync(
    "git",
    ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    { cwd: process.cwd(), encoding: "utf8" },
  ).trim();
  const repositoryRoot = path.dirname(commonDir);
  return path.join(repositoryRoot, ".codefactory-cache", "cargo-target");
}

const targetDir = resolveSharedTarget();
const args = process.argv.slice(2);

if (args.length === 1 && args[0] === "--print-target") {
  process.stdout.write(`${targetDir}\n`);
  process.exit(0);
}

if (args.length === 0) {
  process.stderr.write("Usage: pnpm cargo:shared -- <cargo arguments>\n");
  process.exit(2);
}

const result = spawnSync("cargo", args, {
  cwd: process.cwd(),
  env: { ...process.env, CARGO_TARGET_DIR: targetDir },
  stdio: "inherit",
});

if (result.error) {
  throw result.error;
}
process.exit(result.status ?? 1);
