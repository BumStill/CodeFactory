// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// 控件边界对比度守卫。
//
// `--control-border-color` 画的是输入框这类控件的外框。在浅色主题下控件填充
// (surface-2, 纯白) 与页面底 (surface-0, #EFF3F8) 的对比只有 1.08:1——
// 也就是说**边框本身承担了「这里是一个控件」的全部信息**，WCAG 1.4.11
// 要求它对**两侧相邻颜色**都达到 3:1。
//
// 这条守卫同时卡上下限：
//   - 低于 3:1 → 无障碍不合格
//   - 高于 4.5:1 → 视觉过重。两个主题曾共用一个值，浅色下 4.76:1，
//     在大圆角输入框上读起来是一道深色描边而不是边界。
//
// 见 docs/specs/ui-typography-and-spacing.md。

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const globals = readFileSync(resolve(process.cwd(), "src/styles/globals.css"), "utf8");

/** WCAG 1.4.11 的下限。 */
const MIN_RATIO = 3;
/**
 * 上限。不是无障碍要求，是设计约束：边界的职责是划分，不是强调。
 * 超过这个值就该问问是不是把描边当成了装饰。
 */
const MAX_RATIO = 4.5;

function relativeLuminance([red, green, blue]: number[]): number {
  const [r, g, b] = [red, green, blue].map((channel) => {
    const value = channel / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(foreground: number[], background: number[]): number {
  const a = relativeLuminance(foreground);
  const b = relativeLuminance(background);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

/** 读出某个主题块里某个变量的 RGB 三元组。 */
function readVariable(themeSelector: string, name: string): number[] {
  const block = globals.slice(globals.indexOf(themeSelector));
  const match = block.match(new RegExp(`${name}:\\s*(\\d+)\\s+(\\d+)\\s+(\\d+)`));
  if (!match) throw new Error(`在 ${themeSelector} 里找不到 ${name}`);
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

const THEMES = [
  { name: "深色", selector: ":root,", },
  { name: "浅色", selector: '[data-theme="light"]' },
];

describe("控件边界对比度", () => {
  for (const theme of THEMES) {
    it(`${theme.name}主题：控件边框对两侧相邻色都在 ${MIN_RATIO}–${MAX_RATIO}:1 之间`, () => {
      const border = readVariable(theme.selector, "--control-border-color");
      // 两侧：控件自身填充，以及它背后的页面。
      for (const neighbour of ["--surface-2", "--surface-0"]) {
        const ratio = contrast(border, readVariable(theme.selector, neighbour));
        expect(
          ratio,
          `${theme.name} 控件边框 vs ${neighbour} 是 ${ratio.toFixed(2)}:1，` +
            `低于 ${MIN_RATIO}:1 不合无障碍，高于 ${MAX_RATIO}:1 视觉过重`,
        ).toBeGreaterThanOrEqual(MIN_RATIO);
        expect(ratio).toBeLessThanOrEqual(MAX_RATIO);
      }
    });
  }

  it("两个主题使用各自的值，不共用一个", () => {
    const dark = readVariable(":root,", "--control-border-color");
    const light = readVariable('[data-theme="light"]', "--control-border-color");
    expect(
      dark.join(),
      "深浅两个主题的背景亮度相反，同一个边框色不可能在两边都恰好达标——共用值意味着至少有一边要么不合格要么过重",
    ).not.toBe(light.join());
  });
});
