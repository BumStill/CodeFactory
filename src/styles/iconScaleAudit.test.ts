// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// 图标比例尺守卫。
//
// 产品原先用了 18 种图标尺寸：6,7,8,9,10,11,12,13,14,15,16,17,18,20,22,23,24,32。
// 除了「同一语义的图标在不同页面大小不同」之外，奇数尺寸还有一个渲染问题：
// lucide 的 viewBox 是 24、默认 strokeWidth 是 2，所以 size={11} 的实际描边是
// 11/24×2 = 0.917px，size={13} 是 1.083px。在 1× 显示器上前者被渲染成灰色模糊线，
// 后者是清晰实线——同一排图标粗细不一致。
//
// 描边一致性由 globals.css 的 `.lucide { vector-effect: non-scaling-stroke }` 统一处理；
// 本守卫只管尺寸档位。
//
// 见 docs/specs/ui-typography-and-spacing.md 第四节。

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const SRC = resolve(process.cwd(), "src");

/** 四档，全部为偶数。与规范第四节保持一致。 */
export const ICON_SIZES = [14, 16, 20, 24];

const SIZE_PROP = /\bsize=\{(\d+)\}/g;

function tsxFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) tsxFiles(full, acc);
    else if (entry.endsWith(".tsx")) acc.push(full);
  }
  return acc;
}

describe("图标比例尺", () => {
  it("size 只使用规范定义的四档", () => {
    const violations: string[] = [];
    for (const file of tsxFiles(SRC)) {
      const rel = relative(SRC, file).replace(/\\/g, "/");
      readFileSync(file, "utf8").split("\n").forEach((line, index) => {
        for (const match of line.matchAll(SIZE_PROP)) {
          const size = Number(match[1]);
          if (!ICON_SIZES.includes(size)) violations.push(`  ${rel}:${index + 1}  size={${size}}`);
        }
      });
    }
    expect(
      violations,
      `共 ${violations.length} 处。只允许 ${ICON_SIZES.join(" / ")}；需要状态点这类装饰图形请用 CSS 元素而不是缩小图标：\n${violations.join("\n")}`,
    ).toEqual([]);
  });

  it("globals.css 统一了图标描边，使不同尺寸的视觉粗细一致", () => {
    const globals = readFileSync(resolve(process.cwd(), "src/styles/globals.css"), "utf8");
    expect(globals, "缺少 .lucide 的描边规则").toMatch(/\.lucide\b/);
    expect(globals, "缺少 vector-effect: non-scaling-stroke").toMatch(/non-scaling-stroke/);
  });
});
