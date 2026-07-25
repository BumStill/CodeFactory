// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it } from "vitest";
import postcss from "postcss";
import tailwindcss from "tailwindcss";
import loadConfig from "tailwindcss/loadConfig.js";

describe("semantic theme color opacity", () => {
  it("compiles opacity modifiers for semantic CSS-variable colors", async () => {
    const config = loadConfig(`${process.cwd()}/tailwind.config.js`);
    const result = await postcss([
      tailwindcss({
        theme: config.theme,
        content: [
          {
            raw: '<div class="border-border/25 bg-surface-1/30 text-gray-500/70 text-accent/60"></div>',
          },
        ],
      }),
    ]).process("@tailwind utilities;", { from: undefined });

    expect(result.css).toContain(".border-border\\/25");
    expect(result.css).toContain(".bg-surface-1\\/30");
    expect(result.css).toContain(".text-gray-500\\/70");
    expect(result.css).toContain(".text-accent\\/60");
    expect(result.css).toContain("rgb(var(--border-color) / 0.25)");
  });
});
