// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// 圆角比例尺守卫。
//
// 产品原先四档混用：`rounded` 381 处、`rounded-lg` 141、`rounded-md` 25、`rounded-xl` 22。
// `rounded`(4px) 与 `rounded-md`(6px) 之间差 2px，肉眼无法区分，只是噪音。
//
// 注意 `rounded-2xl`(16px) **保留**：它与 `rounded-xl`(12px) 差 4px 是能分辨的，
// 而且消息气泡和输入框合理需要更大的圆角。初审时把它一并划入删除是判断错误。
//
// 见 docs/specs/ui-typography-and-spacing.md 第五节。

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const SRC = resolve(process.cwd(), "src");

/** 允许的圆角档位（`rounded-full` 与方向性变体如 `rounded-br-sm` 另计）。 */
export const RADII = ["rounded", "rounded-lg", "rounded-xl", "rounded-2xl", "rounded-full", "rounded-none"];

/** 被本规范淘汰的档位。 */
const RETIRED = /\brounded-(md|sm|3xl)\b/g;

function tsxFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) tsxFiles(full, acc);
    else if (entry.endsWith(".tsx")) acc.push(full);
  }
  return acc;
}

describe("圆角比例尺", () => {
  it("不使用被淘汰的圆角档位", () => {
    const violations: string[] = [];
    for (const file of tsxFiles(SRC)) {
      const rel = relative(SRC, file).replace(/\\/g, "/");
      readFileSync(file, "utf8").split("\n").forEach((line, index) => {
        for (const match of line.matchAll(RETIRED)) {
          // 方向性变体（rounded-br-sm 之类）是为了做「气泡尖角」，不在收敛范围内。
          if (/\brounded-(t|b|l|r|tl|tr|bl|br)-/.test(match.input ?? "")) continue;
          violations.push(`  ${rel}:${index + 1}  ${match[0]}`);
        }
      });
    }
    expect(
      violations,
      `共 ${violations.length} 处。只用 ${RADII.join(" / ")}：\n${violations.join("\n")}`,
    ).toEqual([]);
  });
});
