// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// 字体栈守卫。
//
// 两件事曾经同时成立：设置页提供「Inter」选项，而仓库里没有任何 woff2、
// 没有 @font-face——Inter 从来没被打包过。用户机器上没装 Inter（Windows 上
// 几乎必然没有）时，这个选项静默退化成 system-ui，与「System UI」完全一样，
// 连预览文字都长得一模一样。
//
// 另外这是一个 lang="zh-CN" 的产品，字体栈里却没有中文族，中文用什么字体
// 完全交给 system-ui 兜底，跨平台不可控。
//
// 见 docs/specs/ui-typography-and-spacing.md 第三节。

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { FONT_FAMILIES, MONO_FONT_FAMILIES } from "../stores/settings";

const globals = readFileSync(resolve(process.cwd(), "src/styles/globals.css"), "utf8");
const packageJson = JSON.parse(readFileSync(resolve(process.cwd(), "package.json"), "utf8"));

/** 至少要有一个中文族兜在系统之前。 */
const CJK_FAMILIES = ["PingFang SC", "Microsoft YaHei UI", "Noto Sans SC", "Source Han Sans"];

/**
 * 不需要打包的通用族名——它们由平台提供，写进栈里不构成「承诺了拿不出的东西」。
 */
const SYSTEM_GENERICS = [
  "-apple-system",
  "system-ui",
  "ui-monospace",
  "sans-serif",
  "monospace",
  "SF Mono",
  "Consolas",
  "Menlo",
  ...CJK_FAMILIES,
];

/** 栈里第一个具名族，也就是这个选项真正承诺的字体。 */
function primaryFamily(stack: string): string {
  return (stack.split(",")[0] ?? "").trim().replace(/^['"]|['"]$/g, "");
}

describe("字体栈", () => {
  it("界面字体栈显式包含中文族", () => {
    for (const [key, stack] of Object.entries(FONT_FAMILIES)) {
      const hasCjk = CJK_FAMILIES.some((family) => stack.includes(family));
      expect(hasCjk, `UI 字体选项 "${key}" 的栈里没有中文族，中文将由平台任意兜底：${stack}`).toBe(true);
    }
  });

  it("每个被提供的字体选项要么随应用打包，要么是系统通用族", () => {
    const declared = [...Object.values(FONT_FAMILIES), ...Object.values(MONO_FONT_FAMILIES)];
    for (const stack of declared) {
      const primary = primaryFamily(stack);
      if (SYSTEM_GENERICS.includes(primary)) continue;
      // 打包过的族名会出现在 globals.css 引入的 fontsource 包名里。
      const slug = primary.toLowerCase().replace(/ variable$/, "").replace(/\s+/g, "-");
      expect(
        globals.includes(`@fontsource-variable/${slug}`),
        `字体选项承诺了 "${primary}"，但它没有随应用打包——用户机器上没装就会静默退化`,
      ).toBe(true);
      expect(
        Object.keys(packageJson.dependencies ?? {}).includes(`@fontsource-variable/${slug}`),
        `"${primary}" 未在 dependencies 中声明`,
      ).toBe(true);
    }
  });

  it("界面字体选项里不提供等宽字体", () => {
    for (const [key, stack] of Object.entries(FONT_FAMILIES)) {
      expect(
        /mono/i.test(stack),
        `UI 字体选项 "${key}" 是等宽字体。等宽族没有中文字形，选中后界面会变成等宽拉丁 + 比例中文的混合体；等宽字体是独立设置项。`,
      ).toBe(false);
    }
  });
});
