// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

function source(path: string): string {
  return readFileSync(resolve(process.cwd(), path), "utf8");
}

describe("composer and model picker visual hierarchy", () => {
  it("uses a single subtle composer outline without a blue focus ring", () => {
    const input = source("src/components/MessageInput.tsx");

    expect(input).toContain("rounded-2xl border border-border/40 bg-surface-2 shadow-sm");
    expect(input).not.toContain("focus-within:border-accent");
  });

  it("does not duplicate context percentage with a decorative meter", () => {
    const contextBar = source("src/components/ContextUsageBar.tsx");
    expect(contextBar).not.toContain("h-1.5 w-24 overflow-hidden rounded-full");
    expect(contextBar).not.toContain("presentation.barClass");
    expect(contextBar).toContain('data-testid="context-usage-ring"');
  });

  it("keeps model picker borders and separators subtle", () => {
    const picker = source("src/components/ModelPicker.tsx");
    expect(picker).toContain("border border-border/50 bg-surface-2 shadow-lg");
    expect(picker).toContain("border-b border-border/35");
    expect(picker).not.toContain("border border-border bg-surface-2 shadow-xl");
  });
});
