// SPDX-License-Identifier: Apache-2.0
//
// CF-BLD-R4: reclaiming the build cache must be reachable from the background
// entry point, not only from the Resources panel's button. These cases pin the
// three layers that have to agree: the native protocol, the CLI allow-list and
// the front-end translation layer.

import { describe, it, expect, vi } from "vitest";
import { readFileSync } from "node:fs";

import { applyDispatchRequest, type DispatchHandlers } from "./desktopDispatch";

function handlers(overrides: Partial<DispatchHandlers> = {}): DispatchHandlers {
  return {
    createAndSend: vi.fn(),
    send: vi.fn(),
    setPermission: vi.fn(),
    status: vi.fn(),
    setModel: vi.fn(),
    switchSession: vi.fn(),
    stop: vi.fn(),
    listApprovals: vi.fn(),
    resolveApproval: vi.fn(),
    cleanBuildCache: vi.fn(async () => ({ reclaimed_bytes: 42 })),
    ...overrides,
  };
}

describe("clean_build_cache", () => {
  it("forwards a fieldless cleanup request to the same action the panel uses", async () => {
    const h = handlers();
    const outcome = await applyDispatchRequest(
      { request_id: "r-1", operation: "clean_build_cache", request: {} },
      h,
    );
    expect(outcome).toEqual({ ok: true, result: { reclaimed_bytes: 42 } });
    expect(h.cleanBuildCache).toHaveBeenCalledTimes(1);
  });

  it("refuses to pretend when the app has not wired the action", async () => {
    const h = handlers({ cleanBuildCache: undefined });
    const outcome = await applyDispatchRequest(
      { request_id: "r-2", operation: "clean_build_cache", request: {} },
      h,
    );
    expect(outcome.ok).toBe(false);
  });

  it("is advertised by the native protocol and the CLI allow-list", () => {
    const rust = readFileSync("src-tauri/src/headless_dispatch.rs", "utf8");
    expect(rust).toContain("CleanBuildCache {}");
    expect(rust).toContain('"clean_build_cache"');
    const cli = readFileSync("scripts/dispatch-task.mjs", "utf8");
    expect(cli).toContain('"clean_build_cache",');
  });
});
