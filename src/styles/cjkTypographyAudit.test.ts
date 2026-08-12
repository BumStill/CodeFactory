// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// 中文排版守卫。
//
// 产品有 49 处把英文的「SMALL CAPS 分组标签」惯用法直接套在中文上：
// `uppercase` 对中文无效，只会把混排其中的英文意外大写——同一个词渲染成
// Token / TOKEN / Tokens / TOKENS 四种；`tracking-wider` 对中文有害，
// 中文字本身是等宽方块，再加字间距会把词组拆散（实测「可以试试」被拉开 0.275px）。
//
// text-transform 与 letter-spacing 都会继承，所以只要元素内部任何位置出现中文，
// 这两个属性就是违规的。
//
// 规范：含中文的文本节点禁止 uppercase 和 tracking-*；分组小标题靠字重、颜色和
// 间距建立层级。uppercase 仅允许用于确认不含中文的纯拉丁短标签。
//
// 见 docs/specs/ui-typography-and-spacing.md 第七节。

import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const SRC = resolve(process.cwd(), "src");

const CJK = /[　-〿㐀-䶿一-鿿豈-﫿＀-￯]/;

/** 标签名起始位置。属性区间由 scanOpenTag 走完，不用正则。 */
const TAG_START = /<([A-Za-z][\w.]*)/g;
const STYLED = /\buppercase\b|\btracking-/;
/** 元素内容的检查窗口，够覆盖一屏 JSX，又让扫描保持线性。 */
const CONTENT_WINDOW = 400;

/**
 * 从标签名之后走到开标签的 `>`，返回属性文本与结束位置。
 *
 * 不能用 `[^>]*`：JSX 属性里的箭头函数 `onClick={() => ...}` 自带一个 `>`，
 * 正则会在那里提前收尾，于是所有带箭头函数的元素都被漏掉——
 * `CheckpointsPanel.tsx` 里那个「无文件差异」按钮就是这样躲过第一版守卫的。
 * 这里跟踪花括号深度和引号状态，只认真正闭合开标签的那个 `>`。
 */
function scanOpenTag(source: string, from: number): { attributes: string; end: number } | null {
  let depth = 0;
  let quote: string | null = null;
  for (let i = from; i < source.length; i += 1) {
    const char = source[i];
    if (quote) {
      if (char === quote && source[i - 1] !== "\\") quote = null;
      continue;
    }
    if (char === '"' || char === "'" || char === "`") { quote = char; continue; }
    if (char === "{") { depth += 1; continue; }
    if (char === "}") { depth -= 1; continue; }
    if (char === ">" && depth === 0) {
      return { attributes: source.slice(from, i), end: i + 1 };
    }
    if (char === "<" && depth === 0) return null; // 未闭合，放弃
  }
  return null;
}

function tsxFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) tsxFiles(full, acc);
    else if (entry.endsWith(".tsx")) acc.push(full);
  }
  return acc;
}

/**
 * 只看真正会被渲染成文字的内容：剥掉嵌套标签、JSX 表达式和注释。
 *
 * 表达式内容（如 `{label}`）无法静态判定语言，不在本守卫范围内——
 * 它们由术语表和实地验证负责。
 */
function renderedText(inner: string): string {
  return inner.replace(/\{[^{}]*\}/g, " ").replace(/<[^>]*>/g, " ");
}

interface Violation {
  file: string;
  line: number;
  property: string;
  text: string;
}

function scan(): Violation[] {
  const violations: Violation[] = [];
  for (const file of tsxFiles(SRC)) {
    const rel = relative(SRC, file).replace(/\\/g, "/");
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(TAG_START)) {
      const tagStart = match.index ?? 0;
      const open = scanOpenTag(source, tagStart + match[0].length);
      if (!open || !STYLED.test(open.attributes)) continue;
      const window = source.slice(open.end, open.end + CONTENT_WINDOW);
      // 只取到本元素闭合为止，避免把后续兄弟节点的中文算进来。
      const closing = window.indexOf(`</${match[1]}>`);
      const inner = closing === -1 ? window : window.slice(0, closing);
      const text = renderedText(inner);
      if (!CJK.test(text)) continue;
      violations.push({
        file: rel,
        line: source.slice(0, tagStart).split("\n").length,
        property: /\buppercase\b/.test(open.attributes) ? "uppercase" : "tracking-*",
        text: text.replace(/\s+/g, " ").trim().slice(0, 20),
      });
    }
  }
  return violations;
}

describe("中文排版", () => {
  it("含中文的元素不使用 uppercase 或 tracking-*", () => {
    const violations = scan();
    const detail = violations
      .map((v) => `  ${v.file}:${v.line}  ${v.property}  「${v.text}」`)
      .join("\n");
    expect(
      violations,
      `共 ${violations.length} 处。uppercase 对中文无效只会误伤混排英文，tracking-* 会把中文词组拆散：\n${detail}`,
    ).toEqual([]);
  });
});
