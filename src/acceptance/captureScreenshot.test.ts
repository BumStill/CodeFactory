// SPDX-License-Identifier: Apache-2.0
// Evidence capture must never be the reason a headless acceptance gate goes red.
// Windows Chromium intermittently stalls on a full-page PNG capture (run
// 39723529947: every assertion passed, then `page.screenshot` timed out after
// 30s and the run published no artifact at all). The capture helper keeps the
// assertion contract untouched while guaranteeing that a gate that passed still
// leaves evidence behind.
import { describe, expect, it, vi } from "vitest";

import { captureScreenshot } from "./captureScreenshot.mjs";

type Capture = (options: Record<string, unknown>) => Promise<void>;

function fakePage(behaviour: (attempt: number, options: Record<string, unknown>) => Promise<void>) {
  const calls: Array<Record<string, unknown>> = [];
  const page = {
    screenshot: vi.fn(async (options: Record<string, unknown>) => {
      calls.push(options);
      return behaviour(calls.length, options);
    }) as unknown as Capture,
  };
  return { page, calls };
}

describe("captureScreenshot", () => {
  it("captures the full page with a bounded, animation-free screenshot", async () => {
    const { page, calls } = fakePage(async () => {});
    const result = await captureScreenshot(page as never, "/tmp/evidence.png", { log: () => {} });

    expect(result.fullPage).toBe(true);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ path: "/tmp/evidence.png", fullPage: true, animations: "disabled" });
    expect(Number(calls[0].timeout)).toBeGreaterThanOrEqual(30_000);
  });

  it("falls back to a viewport frame when the full-page capture times out", async () => {
    const { page, calls } = fakePage(async (attempt) => {
      if (attempt === 1) throw new Error("page.screenshot: Timeout 30000ms exceeded.");
    });
    const warnings: string[] = [];

    const result = await captureScreenshot(page as never, "/tmp/evidence.png", { log: (line: string) => warnings.push(line) });

    expect(result.fullPage).toBe(false);
    expect(calls).toHaveLength(2);
    expect(calls[1]).toMatchObject({ path: "/tmp/evidence.png", fullPage: false });
    expect(warnings.join("\n")).toContain("Timeout 30000ms exceeded");
  });

  it("still fails loudly when no evidence could be written at all", async () => {
    const { page, calls } = fakePage(async () => {
      throw new Error("page.screenshot: Target page, context or browser has been closed");
    });

    await expect(captureScreenshot(page as never, "/tmp/evidence.png", { log: () => {} }))
      .rejects.toThrow(/has been closed/);
    expect(calls).toHaveLength(2);
  });
});
