// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  download: vi.fn(),
  install: vi.fn(),
  relaunch: vi.fn(),
  getVersion: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: mocks.relaunch }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: mocks.getVersion }));
vi.mock("../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

import { useUpdaterStore } from "./updater";

describe("updater safe restart gate", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllTimers();
    mocks.check.mockReset();
    mocks.download.mockReset().mockResolvedValue(undefined);
    mocks.install.mockReset().mockResolvedValue(undefined);
    mocks.relaunch.mockReset().mockResolvedValue(undefined);
    mocks.getVersion.mockReset().mockResolvedValue("1.79.0");
    mocks.invoke.mockReset();
    useUpdaterStore.setState({
      phase: { kind: "idle" },
      currentVersion: "1.79.0",
      pollHandle: null,
      safeRetryHandle: null,
      dismissedVersion: null,
    });
  });

  it("does not install or relaunch while a local session is executing", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue({
      safe_to_restart: false,
      restart_reserved: false,
      active_chat_turns: 1,
      active_task_schedulers: 0,
      active_delivery_leases: 0,
      pending_permissions: 0,
      managed_browser_sessions: 0,
      terminal_sessions: 0,
    });

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.invoke).toHaveBeenCalledWith("reserve_update_install");
    expect(mocks.download).toHaveBeenCalledOnce();
    expect(mocks.install).not.toHaveBeenCalled();
    expect(mocks.relaunch).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "waiting_for_safe_restart",
      update,
      blockers: { active_chat_turns: 1 },
    });
  });

  it("automatically installs after the executing session reaches a safe point", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke
      .mockResolvedValueOnce({
        safe_to_restart: false,
        restart_reserved: false,
        active_chat_turns: 1,
        active_task_schedulers: 0,
        active_delivery_leases: 0,
        pending_permissions: 0,
        managed_browser_sessions: 0,
        terminal_sessions: 0,
      })
      .mockResolvedValueOnce({
        safe_to_restart: true,
        restart_reserved: true,
        active_chat_turns: 0,
        active_task_schedulers: 0,
        active_delivery_leases: 0,
        pending_permissions: 0,
        managed_browser_sessions: 0,
        terminal_sessions: 0,
      });

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();
    expect(useUpdaterStore.getState().phase.kind).toBe("waiting_for_safe_restart");

    await vi.advanceTimersByTimeAsync(5_000);

    expect(mocks.download).toHaveBeenCalledOnce();
    expect(mocks.install).toHaveBeenCalledOnce();
    expect(useUpdaterStore.getState().phase.kind).toBe("ready");
    await vi.advanceTimersByTimeAsync(800);
    expect(mocks.relaunch).toHaveBeenCalledOnce();
  });

  it("fails closed and retries when the safety snapshot is unavailable", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockRejectedValue(new Error("safety database unavailable"));

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.install).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "waiting_for_safe_restart",
      blockers: null,
      safetyCheckError: "safety database unavailable",
    });
  });

  it("releases the restart reservation when installation fails", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      download: mocks.download,
      install: mocks.install.mockRejectedValue(new Error("installer failed")),
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue({
      safe_to_restart: true,
      restart_reserved: true,
      active_chat_turns: 0,
      active_task_schedulers: 0,
      active_delivery_leases: 0,
      pending_permissions: 0,
      managed_browser_sessions: 0,
      terminal_sessions: 0,
    });

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "reserve_update_install");
    expect(mocks.invoke).toHaveBeenNthCalledWith(2, "release_update_install_reservation");
    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "error",
      message: "installer failed",
    });
  });
});
