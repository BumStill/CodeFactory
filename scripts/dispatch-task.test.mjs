// SPDX-License-Identifier: Apache-2.0
//
// 可运行示例（`scripts/dispatch-task.mjs`）的验收：它必须说对协议、把安全规则
// 挡在连接之前，并把失败如实报出来（CF-HDE-R2/R3/R5）。
//
// 服务端在本进程内起，客户端用子进程跑，所以这里是真实的两条进程、真实的
// Unix socket 往返，而不是把逻辑再实现一遍。

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SCRIPT = join(process.cwd(), "scripts/dispatch-task.mjs");

/** 跑一次示例脚本，收集退出码和输出。 */
function runExample(requestJson, env) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [SCRIPT, requestJson], {
      env: { ...process.env, ...env },
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("close", (code) => resolve({ code, stdout, stderr }));
  });
}

/** 起一个只关心协议形状的本地 stub 服务端，返回 {dir, close}。 */
function startStubServer(reply) {
  const dir = mkdtempSync(join(tmpdir(), "cf-dispatch-"));
  const socketPath = join(dir, "dispatch.sock");
  const server = createServer((connection) => {
    let buffer = "";
    connection.on("data", (chunk) => {
      buffer += chunk;
      const newline = buffer.indexOf("\n");
      if (newline < 0) return;
      const request = JSON.parse(buffer.slice(0, newline));
      connection.end(`${JSON.stringify(reply(request))}\n`);
    });
  });
  return new Promise((resolve) => {
    server.listen(socketPath, () => {
      chmodSync(socketPath, 0o600);
      resolve({
        dir,
        close: () =>
          new Promise((done) => {
            server.close(() => {
              rmSync(dir, { recursive: true, force: true });
              done();
            });
          }),
      });
    });
  });
}

test("交付授权缺失时在连接之前就被拒绝（M48）", async () => {
  const result = await runExample(
    '{"operation":"send","session_id":"s-1","message":"please open a PR and merge it"}',
    { CODEFACTORY_DISPATCH_DIR: join(tmpdir(), "cf-dispatch-absent") },
  );
  assert.equal(result.code, 1);
  assert.match(result.stderr, /delivery_authorized must be explicit/);
});

test("未知操作不会被猜成最接近的那个", async () => {
  const result = await runExample('{"operation":"exec","cmd":"id"}', {
    CODEFACTORY_DISPATCH_DIR: join(tmpdir(), "cf-dispatch-absent"),
  });
  assert.equal(result.code, 1);
  assert.match(result.stderr, /unsupported operation/);
});

test("真实 socket 往返：成功应答原样输出并以 0 退出", async () => {
  const server = await startStubServer((request) => ({
    ok: true,
    result: { echo: request.operation, session_id: request.session_id },
  }));
  try {
    const result = await runExample('{"operation":"status","session_id":"s-1"}', {
      CODEFACTORY_DISPATCH_DIR: server.dir,
    });
    assert.equal(result.code, 0);
    const reply = JSON.parse(result.stdout);
    assert.equal(reply.ok, true);
    assert.equal(reply.result.echo, "status");
    assert.equal(reply.result.session_id, "s-1");
  } finally {
    await server.close();
  }
});

test("失败应答不会被包装成成功", async () => {
  const server = await startStubServer(() => ({
    ok: false,
    error: { code: "not_found", message: "session not found" },
  }));
  try {
    const result = await runExample('{"operation":"status","session_id":"missing"}', {
      CODEFACTORY_DISPATCH_DIR: server.dir,
    });
    assert.equal(result.code, 1);
    assert.equal(JSON.parse(result.stdout).error.code, "not_found");
  } finally {
    await server.close();
  }
});

test("应用不在时给出明确错误，而不是谎报成功", async () => {
  const result = await runExample('{"operation":"stop"}', {
    CODEFACTORY_DISPATCH_DIR: join(tmpdir(), "cf-dispatch-absent"),
  });
  assert.equal(result.code, 2);
  assert.match(result.stderr, /not accepting local tasks/);
});
