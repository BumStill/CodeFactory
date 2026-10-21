// SPDX-License-Identifier: Apache-2.0
// 覆盖 verify-background-tasks-headless.mjs 的 esbuild 二进制查找逻辑。
//
// 该门禁在 check-frontend 里跑在 Windows runner 上，而它原先只认
// `@esbuild/<platform>/bin/esbuild`：Windows 的平台包把可执行文件放在包根目录
// （`esbuild.exe`），于是 CI 直接报 "no esbuild binary found"。这里用合成目录
// 复现两种真实布局，确保查找逻辑不依赖运行测试的机器本身。

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import {
  esbuildBinaryLayouts,
  esbuildExecutableName,
  findEsbuildBinary,
} from "./verify-background-tasks-headless.mjs";

async function makeRoot() {
  const root = await mkdtemp(path.join(os.tmpdir(), "cf-esbuild-find-"));
  await mkdir(path.join(root, "node_modules", ".pnpm"), { recursive: true });
  return root;
}

// 包名形如 win32-x64 / darwin-arm64，而查找逻辑按 process.platform 取值（win32 / darwin / linux）。
function osPlatformOf(packageName) {
  return packageName.split("-")[0];
}

// 按给定平台的真实包布局放一个可执行文件进去。
async function placePlatformPackage(root, platform, { layout = "native" } = {}) {
  const pkgDir = path.join(
    root, "node_modules", ".pnpm", `@esbuild+${platform}@0.21.5`,
    "node_modules", "@esbuild", platform,
  );
  const executable = esbuildExecutableName(osPlatformOf(platform));
  const relative = layout === "native"
    ? esbuildBinaryLayouts(osPlatformOf(platform))[0]
    : path.join("bin", executable);
  const binary = path.join(pkgDir, relative);
  await mkdir(path.dirname(binary), { recursive: true });
  await writeFile(binary, "#!/bin/sh\nexit 0\n", "utf8");
  return binary;
}

test("windows platform package resolves esbuild.exe at the package root", async (t) => {
  const root = await makeRoot();
  t.after(() => rm(root, { recursive: true, force: true }));
  const expected = await placePlatformPackage(root, "win32-x64");

  // 前提：Windows 布局确实是包根目录，而不是 bin/ 下。
  assert.equal(path.basename(expected), "esbuild.exe");
  assert.equal(path.dirname(expected), path.join(
    root, "node_modules", ".pnpm", "@esbuild+win32-x64@0.21.5",
    "node_modules", "@esbuild", "win32-x64",
  ));

  const found = await findEsbuildBinary({ searchRoot: root, platform: "win32" });
  assert.equal(found, expected);
});

test("unix platform package resolves the binary under bin/", async (t) => {
  const root = await makeRoot();
  t.after(() => rm(root, { recursive: true, force: true }));
  const expected = await placePlatformPackage(root, "darwin-arm64");

  assert.equal(path.basename(path.dirname(expected)), "bin");
  const found = await findEsbuildBinary({ searchRoot: root, platform: "darwin" });
  assert.equal(found, expected);
});

test("native platform wins when several platform packages are installed", async (t) => {
  const root = await makeRoot();
  t.after(() => rm(root, { recursive: true, force: true }));
  await placePlatformPackage(root, "darwin-arm64");
  const windowsBinary = await placePlatformPackage(root, "win32-x64");

  const found = await findEsbuildBinary({ searchRoot: root, platform: "win32" });
  assert.equal(found, windowsBinary);
});

test("node_modules/.bin executable takes precedence over the pnpm store", async (t) => {
  const root = await makeRoot();
  t.after(() => rm(root, { recursive: true, force: true }));
  await placePlatformPackage(root, "win32-x64");
  const shim = path.join(root, "node_modules", ".bin", "esbuild.exe");
  await mkdir(path.dirname(shim), { recursive: true });
  await writeFile(shim, "", "utf8");

  const found = await findEsbuildBinary({ searchRoot: root, platform: "win32" });
  assert.equal(found, shim);
});

test("a missing esbuild install fails with the platform named in the message", async (t) => {
  const root = await makeRoot();
  t.after(() => rm(root, { recursive: true, force: true }));

  await assert.rejects(
    () => findEsbuildBinary({ searchRoot: root, platform: "win32" }),
    /no esbuild\.exe binary found under node_modules for win32/,
  );
});
