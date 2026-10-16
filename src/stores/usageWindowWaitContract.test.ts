// SPDX-License-Identifier: Apache-2.0
// CF-WINDOW-R5（M86 返修）：后端把"等待"作为 StreamEvent::TransportRetry 发出，
// 前端靠 label 把它和真正的连接重试区分开。两侧的标签必须是同一个字符串，
// 否则等待说明会退化成折叠在"模型连接不稳定"里的日志行。
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import path from "node:path";
import { USAGE_WINDOW_WAIT_LABEL } from "./chatEvents";

const failoverSource = readFileSync(
  path.resolve(__dirname, "../../src-tauri/src/agent/failover.rs"),
  "utf8",
);

describe("usage-window wait label contract", () => {
  it("uses exactly one label for the usage-window wait on both sides", () => {
    expect(USAGE_WINDOW_WAIT_LABEL).toBe("模型使用时段等待");
    expect(failoverSource).toContain(`label: "${USAGE_WINDOW_WAIT_LABEL}"`);
  });
});
