// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// 首屏主题守卫。
//
// 主题变量原先只定义在 [data-theme="dark"] 和 [data-theme="light"] 下，:root 上没有兜底；
// 而 data-theme 属性要等 get_settings 这个异步 IPC 返回后才由 stores/settings.ts 写上。
// 在此之前 --surface-0 未定义，`background-color: rgb(var(--surface-0))` 是无效声明，
// 于是每次冷启动都会先画一帧无主题的白底。
//
// index.html 上原本写的 class="dark" 不起任何作用——tailwind.config.js 的 darkMode
// 配置是 ["selector", '[data-theme="dark"]']，看的是属性不是 class。
//
// 见 docs/specs/ui-typography-and-spacing.md 第八节。

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const indexHtml = readFileSync(resolve(process.cwd(), "index.html"), "utf8");
const globals = readFileSync(resolve(process.cwd(), "src/styles/globals.css"), "utf8");
const tailwind = readFileSync(resolve(process.cwd(), "tailwind.config.js"), "utf8");

/** darkMode 选择器所依赖的属性，主题引导必须与它一致。 */
const THEME_ATTRIBUTE = /\[data-theme="dark"\]/;

describe("首屏主题引导", () => {
  it("index.html 在首帧之前就带上 data-theme", () => {
    expect(
      /<html[^>]*\bdata-theme=/.test(indexHtml),
      "index.html 的 <html> 必须直接带 data-theme，否则首帧没有任何主题变量可用",
    ).toBe(true);
  });

  it("index.html 不保留失效的 class=\"dark\"", () => {
    expect(
      /<html[^>]*\bclass="[^"]*\bdark\b/.test(indexHtml),
      'darkMode 配置看的是 [data-theme="dark"] 属性，class="dark" 不起作用，属于误导性标记',
    ).toBe(false);
  });

  it("darkMode 仍然基于 data-theme 属性", () => {
    expect(THEME_ATTRIBUTE.test(tailwind)).toBe(true);
  });

  it("globals.css 在 :root 上提供主题变量兜底", () => {
    // :root 可以单独成规则，也可以与 [data-theme="dark"] 并列在同一个选择器列表里，
    // 后者避免把整套变量抄两遍。两种写法都算兜底。
    //
    // 先剥注释：规则之间的注释会被并进「选择器」那一段捕获，害得 `:root` 永远
    // 匹配不上自己。
    const css = globals.replace(/\/\*[\s\S]*?\*\//g, "");
    // 「选择器」段落里还粘着上一条规则之后的所有 `;` 语句（`@tailwind base;` 之类），
    // 按最后一个 `;` 截断才拿得到真正的选择器列表。
    const selectorsOf = (blob: string): string[] =>
      blob.slice(blob.lastIndexOf(";") + 1).split(",").map((selector) => selector.trim());
    const rule = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].find((match) =>
      selectorsOf(match[1]).includes(":root"),
    );
    expect(rule, "globals.css 需要一个覆盖 :root 的规则，兜住 data-theme 写入之前的那一帧").toBeDefined();
    const body = rule?.[2] ?? "";
    for (const variable of ["--surface-0", "--gray-200", "--border-color", "--accent-color"]) {
      expect(body, `:root 兜底缺少 ${variable}`).toContain(variable);
    }
  });
});
