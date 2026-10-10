#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// 本地派单入口的可运行示例（CF-HDE-R5）。
//
// 用法：
//   node scripts/dispatch-task.mjs '<请求 JSON>'
//   node scripts/dispatch-task.mjs -            # 从 stdin 读请求
//
// 安全边界（和原生侧同一套规则，这里再挡一层，避免把注定失败的请求发出去）：
//   * 交付授权只能来自结构化字段 `delivery_authorized`，绝不从消息文字推断；
//   * 请求体必须正好是协议认识的那些字段；
//   * 只连本机 Unix domain socket，不用任何网络地址。

import { connect } from "node:net";
import { homedir } from "node:os";
import { join } from "node:path";

const OPERATIONS = new Set([
  "create_and_send",
  "send",
  "set_permission",
  "status",
  "set_model",
  "switch_session",
  "stop",
  "list_approvals",
  "resolve_approval",
  "focus_main_display",
  "clean_build_cache",
]);

const PERMISSION_MODES = new Set(["safe", "standard", "trusted"]);

/// CF-HDE-R8：`send` 的方向。省略即 `steer`（界面不加修饰键按 Enter 的语义）。
/// 只有这两个取值——猜错方向等于用户以为在插话、其实在排队。
const SEND_MODES = new Set(["steer", "queue"]);

const MESSAGE_OPERATIONS = new Set(["create_and_send", "send"]);

function fail(code, message) {
  console.error(`${code}: ${message}`);
  process.exit(1);
}

/** 与原生协议一致的字段校验：不合法就根本不发出去。 */
function validate(request) {
  if (request === null || typeof request !== "object" || Array.isArray(request)) {
    fail("invalid_request", "the request must be a JSON object");
  }
  const operation = request.operation;
  if (typeof operation !== "string" || !OPERATIONS.has(operation)) {
    fail("invalid_request", `unsupported operation: ${String(operation)}`);
  }
  if (MESSAGE_OPERATIONS.has(operation)) {
    if (typeof request.message !== "string" || !request.message.trim()) {
      fail("invalid_request", "message must be a non-empty string");
    }
    if (typeof request.delivery_authorized !== "boolean") {
      // 写得再像"请合并这个 PR"也没用：授权只能来自这个字段。
      fail("invalid_request", "delivery_authorized must be explicit");
    }
  }
  if (operation === "set_permission" && !PERMISSION_MODES.has(request.permission_mode)) {
    fail("invalid_request", "permission_mode must be safe, standard or trusted");
  }
  // CF-HDE-R8: 只有 steer（插话引导当前执行）/ queue（本轮结束后再发）两种方向。
  // 省略则按 steer 处理，与原生侧的 `SEND_MODES` 是同一套取值。
  if (operation === "send" && request.mode !== undefined && !SEND_MODES.has(request.mode)) {
    fail("invalid_request", "mode must be steer or queue");
  }
  if (operation === "resolve_approval" && typeof request.approve !== "boolean") {
    fail("invalid_request", "approve must be explicit");
  }
  if (operation !== "list_approvals" && operation !== "focus_main_display") {
    if (typeof request.session_id === "string" && !request.session_id.trim()) {
      fail("invalid_request", "session_id must not be empty");
    }
  }
  return request;
}

function socketPath() {
  const base =
    process.env.CODEFACTORY_DISPATCH_DIR ??
    (process.platform === "darwin"
      ? join(homedir(), "Library/Application Support/com.codefactory.app")
      : join(homedir(), ".config/com.codefactory.app"));
  return join(base, "dispatch.sock");
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

async function main() {
  const arg = process.argv[2];
  if (arg === undefined) {
    fail("invalid_request", "usage: node scripts/dispatch-task.mjs '<request json>' | -");
  }
  const raw = arg === "-" ? await readStdin() : arg;
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    fail("invalid_request", `request is not valid JSON: ${error.message}`);
  }
  const request = validate(parsed);
  const path = socketPath();

  const response = await new Promise((resolve) => {
    const socket = connect(path);
    let buffer = "";
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      if (buffer.includes("\n")) {
        socket.end();
        resolve(buffer.trim());
      }
    });
    socket.on("error", (error) => {
      console.error(
        `internal: CodeFactory is not accepting local tasks (${path}): ${error.message}`,
      );
      process.exit(2);
    });
  });

  let reply;
  try {
    reply = JSON.parse(response);
  } catch (error) {
    console.error(`internal: malformed reply from the app: ${error.message}`);
    process.exit(2);
  }
  console.log(JSON.stringify(reply, null, 2));
  process.exit(reply.ok ? 0 : 1);
}

await main();
