// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// 字号比例尺守卫。
//
// 产品原先 1039 处字号声明里有 868 处（83%）落在 10–11px：`text-xs` 用了 453 处，
// 而在被改写的 rem 基准下它实际只有 10.5px；开发者又用 251 处 `text-[11px]`、
// 164 处 `text-[10px]`、61 处 `text-[13px]` 去绕开这个坏掉的档位。
// 结果是比例尺又平又乱，还出现层级倒挂——欢迎页卡片标题比它自己的正文小，
// 会话消息里的 h3 比它统领的段落小 2.75px。
//
// 规范：业务代码只使用语义字号 token，不使用 Tailwind 原生字号档位，
// 也不使用任意值 text-[Npx]。需要新档位时先改规范。
//
// 见 docs/specs/ui-typography-and-spacing.md 第二节。

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const SRC = resolve(process.cwd(), "src");

/** 规范定义的语义字号 token，与 tailwind.config.js 保持一致。 */
export const TYPOGRAPHY_TOKENS = [
  "caption",
  "label",
  "note",
  "body",
  "reading",
  "title",
  "heading",
  "display",
] as const;

/**
 * 亚 11px 装饰性文字的豁免名单——目前为空，且应当保持为空。
 *
 * 曾经登记过热力图格子里的状态记号（缺失 `×` 7px、零值 `·` 6px、超预算 `!` 7px）。
 * 它们已经改成 CSS 图形：在那个尺寸下字体已经无话可说，画出来比排出来清楚，
 * 而状态本身由格子的 aria-label 承载。新增条目前先想想能不能画。
 */
const DECORATIVE_ALLOWLIST: { file: string; className: string }[] = [];

const RAW_SCALE = /\btext-(xs|sm|base|lg|xl|[2-9]xl)\b/g;
const ARBITRARY = /\btext-\[[0-9.]+(?:px|rem|em)\]/g;

function tsxFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      tsxFiles(full, acc);
    } else if (entry.endsWith(".tsx")) {
      acc.push(full);
    }
  }
  return acc;
}

function allowed(file: string, className: string): boolean {
  return DECORATIVE_ALLOWLIST.some(
    (entry) => file.endsWith(entry.file) && entry.className === className,
  );
}

interface Violation {
  file: string;
  line: number;
  className: string;
}

function scan(pattern: RegExp): Violation[] {
  const violations: Violation[] = [];
  for (const file of tsxFiles(SRC)) {
    const rel = relative(SRC, file).replace(/\\/g, "/");
    readFileSync(file, "utf8").split("\n").forEach((text, index) => {
      for (const match of text.matchAll(pattern)) {
        if (allowed(rel, match[0])) continue;
        violations.push({ file: rel, line: index + 1, className: match[0] });
      }
    });
  }
  return violations;
}

function report(violations: Violation[]): string {
  const byFile = new Map<string, Violation[]>();
  for (const violation of violations) {
    const list = byFile.get(violation.file) ?? [];
    list.push(violation);
    byFile.set(violation.file, list);
  }
  return [...byFile.entries()]
    .map(([file, list]) => `  ${file} (${list.length}): ${list.slice(0, 4).map((v) => `${v.className}@${v.line}`).join(", ")}${list.length > 4 ? " …" : ""}`)
    .join("\n");
}

describe("字号比例尺", () => {
  it("不使用 Tailwind 原生字号档位", () => {
    const violations = scan(RAW_SCALE);
    expect(
      violations,
      `共 ${violations.length} 处。改用语义 token（${TYPOGRAPHY_TOKENS.join(" / ")}）：\n${report(violations)}`,
    ).toEqual([]);
  });

  it("不使用任意值字号", () => {
    const violations = scan(ARBITRARY);
    expect(
      violations,
      `共 ${violations.length} 处。任意值字号是在绕开比例尺；需要新档位请先改 docs/specs/ui-typography-and-spacing.md：\n${report(violations)}`,
    ).toEqual([]);
  });

  it("语义 token 在 tailwind.config.js 中全部有定义", () => {
    const config = readFileSync(resolve(process.cwd(), "tailwind.config.js"), "utf8");
    const missing = TYPOGRAPHY_TOKENS.filter((token) => !new RegExp(`\\b${token}\\s*:`).test(config));
    expect(missing, `tailwind.config.js 缺少字号 token 定义: ${missing.join(", ")}`).toEqual([]);
  });
});
