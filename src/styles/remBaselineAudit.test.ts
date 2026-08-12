// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// rem 基准守卫。
//
// Tailwind 的字号、间距、圆角、宽高全部是 rem，`1rem` 等于 `html` 的字号。
// 应用一旦把 `html` 的 font-size 改成用户字号（曾经是 14px），整套设计系统就被
// 静默乘上 14/16 = 0.875：`text-xs` 渲染成 10.5px、`gap-2` 渲染成 7px、
// `rounded` 渲染成 3.5px，4px 网格不复存在，而且用户调字号时会连带缩放整个布局。
//
// 规范：`html` 的 font-size 必须保持平台默认，用户字号只通过 `--font-scale`
// 驱动字号 token，不触碰 rem 基准。
//
// 见 docs/specs/ui-typography-and-spacing.md 第一节。

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const globals = readFileSync(resolve(process.cwd(), "src/styles/globals.css"), "utf8");
const settings = readFileSync(resolve(process.cwd(), "src/stores/settings.ts"), "utf8");

/** 去掉注释，避免注释里的示例代码触发断言。 */
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, "");
}

describe("rem 基准", () => {
  it("globals.css 不得在 html 选择器上设置 font-size", () => {
    const css = stripComments(globals);
    // 抓出每一条规则的选择器与声明体，检查选择器命中 html 且声明含 font-size。
    const offenders: string[] = [];
    for (const match of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
      const selector = match[1].trim();
      const body = match[2];
      const selectsHtml = selector
        .split(",")
        .some((part) => /(^|\s)html(\s|$|[.:[])/.test(part.trim()));
      if (selectsHtml && /(^|[;\s])font-size\s*:/.test(body)) {
        offenders.push(`${selector} { ${body.trim()} }`);
      }
    }
    expect(
      offenders,
      `html 的 font-size 被覆盖，会把 Tailwind 整套 rem 比例尺连同 4px 网格一起缩放：\n${offenders.join("\n")}`,
    ).toEqual([]);
  });

  it("正文字号通过 --font-scale 表达，而不是覆盖根字号", () => {
    const css = stripComments(globals);
    expect(css, "globals.css 应当在 body 上用 calc(... * var(--font-scale)) 承载正文字号").toMatch(
      /--font-scale/,
    );
  });

  it("设置存储写入 --font-scale，不再写入 --font-size", () => {
    expect(settings).toMatch(/--font-scale/);
    expect(
      settings.includes(`"--font-size"`),
      "用户字号必须只驱动 --font-scale；写回 --font-size 会重新把布局绑上字号",
    ).toBe(false);
  });
});
