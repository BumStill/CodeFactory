// SPDX-License-Identifier: Apache-2.0
//
// 后台无障碍驱动所依赖的两条配置契约。
//
// 1) 主窗口开启 `acceptFirstMouse`:窗口不在前台时,第一下点击必须直接作用到
//    页面上,而不是只把窗口带到前台再吞掉这一下(这正是 M16)。
// 2) 会话按钮的无障碍名称必须带上短 id 与相对时间:同名会话在无障碍树里是
//    一模一样的按钮,读屏软件和后台工具都无法区分。
//
// 这两条都是"配置/文案级"的约定,所以直接对配置文件和组件源码断言。

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { sessionAccessibleName } from "./sessionLabel";

const repoRoot = resolve(__dirname, "../..");

describe("主窗口第一下点击直接生效", () => {
  it("tauri.conf.json 的主窗口开启了 acceptFirstMouse", () => {
    const config = JSON.parse(readFileSync(resolve(repoRoot, "src-tauri/tauri.conf.json"), "utf8"));
    const main = (config.app.windows as { label?: string; acceptFirstMouse?: boolean }[]).find(
      (window) => window.label === "main",
    );
    expect(main, "找不到 label=main 的主窗口").toBeDefined();
    expect(main?.acceptFirstMouse).toBe(true);
  });
});

describe("会话的无障碍名称", () => {
  it("包含短 id 与相对时间", () => {
    const now = 1_000 + 5 * 60_000;
    expect(sessionAccessibleName("新会话", "9537257c-1111-2222-3333", 1_000, now)).toBe(
      "新会话(9537257c,5 分钟前)",
    );
  });

  it("同名会话的无障碍名称互不相同", () => {
    const now = 9_000_000;
    const first = sessionAccessibleName("新会话", "9537257c-aaaa", now - 1_000, now);
    const second = sessionAccessibleName("新会话", "11112222-bbbb", now - 1_000, now);
    expect(first).not.toBe(second);
    expect(first).toContain("9537257c");
    expect(second).toContain("11112222");
  });

  it("侧边栏会话按钮用的是这个名称", () => {
    const sidebar = readFileSync(resolve(repoRoot, "src/components/SessionSidebar.tsx"), "utf8");
    expect(sidebar).toContain("aria-label={`打开会话 ${sessionAccessibleName(");
  });
});
