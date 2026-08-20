// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Update } from "@tauri-apps/plugin-updater";

const mocks = vi.hoisted(() => ({
  check: vi.fn(),
  download: vi.fn(),
  install: vi.fn(),
  relaunch: vi.fn(),
  getVersion: vi.fn(),
  invoke: vi.fn(),
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: mocks.relaunch }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: mocks.getVersion }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

import {
  countUpdateBlockers,
  describeUpdateObjectiveBlockers,
  type UpdateSafetyStatus,
  useUpdaterStore,
} from "./updater";

const TARGET_BUILD = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

function idleSafety(overrides: Partial<UpdateSafetyStatus> = {}): UpdateSafetyStatus {
  return {
    safe_to_restart: true,
    restart_reserved: true,
    active_chat_turns: 0,
    active_task_schedulers: 0,
    active_delivery_leases: 0,
    active_objective_leases: 0,
    objective_blocker_owners: [],
    pending_permissions: 0,
    managed_browser_sessions: 0,
    terminal_sessions: 0,
    update_install_state: "install_permitted",
    ...overrides,
  };
}

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
    mocks.listen.mockReset().mockResolvedValue(() => undefined);
    useUpdaterStore.setState({
      phase: { kind: "idle" },
      currentVersion: "1.79.0",
      pollHandle: null,
      safeRetryHandle: null,
      progressListening: false,
      dismissedVersion: null,
    });
  });

  it("counts active Objective leases and exposes their recovery owners", () => {
    const safety = idleSafety({
      safe_to_restart: false,
      restart_reserved: false,
      active_objective_leases: 2,
      objective_blocker_owners: ["objective-supervisor:chat", "objective-supervisor:provider"],
    });

    expect(countUpdateBlockers(safety)).toBe(2);
    expect(describeUpdateObjectiveBlockers(safety)).toBe(
      "2 个目标仍由 objective-supervisor:chat、objective-supervisor:provider 持有",
    );
  });

  it("does not install or relaunch while a local session is executing", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      rawJson: { build_git_sha: TARGET_BUILD },
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue(idleSafety({
      safe_to_restart: false,
      restart_reserved: false,
      active_chat_turns: 1,
    }));

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.invoke).toHaveBeenCalledWith("reserve_update_install", {
      targetVersion: "1.79.1",
      targetBuild: TARGET_BUILD,
    });
    expect(mocks.download).not.toHaveBeenCalled();
    expect(mocks.install).not.toHaveBeenCalled();
    expect(mocks.relaunch).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "waiting_for_safe_restart",
      update,
      blockers: { active_chat_turns: 1 },
    });
  });

  it("queues update recovery without treating a null frontend permit as install authority", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      rawJson: { build_git_sha: TARGET_BUILD },
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue(idleSafety({
      safe_to_restart: false,
      restart_reserved: false,
      active_chat_turns: 1,
      update_objective_id: "objective-update-queued",
      update_install_state: "queued",
    }));

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();
    expect(useUpdaterStore.getState().phase.kind).toBe("waiting_for_safe_restart");

    expect(mocks.invoke).toHaveBeenCalledWith("reserve_update_install", {
      targetVersion: "1.79.1",
      targetBuild: TARGET_BUILD,
    });
    expect(mocks.download).not.toHaveBeenCalled();
    expect(mocks.install).not.toHaveBeenCalled();
    expect(mocks.relaunch).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase.kind).toBe("waiting_for_safe_restart");
  });

  it("fails closed and retries when the safety snapshot is unavailable", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      rawJson: { build_git_sha: TARGET_BUILD },
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

  it("never accepts install_permitted as renderer authority without an exact permit", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "safe restart gate",
      rawJson: { build_git_sha: TARGET_BUILD },
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue(idleSafety());

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.invoke).toHaveBeenNthCalledWith(1, "reserve_update_install", {
      targetVersion: "1.79.1",
      targetBuild: TARGET_BUILD,
    });
    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    expect(mocks.download).not.toHaveBeenCalled();
    expect(mocks.install).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "waiting_for_safe_restart",
    });
  });

  it("does not replay an install whose durable receipt remains unknown", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "unknown install receipt",
      rawJson: { build_git_sha: TARGET_BUILD },
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    mocks.invoke.mockResolvedValue(
      idleSafety({
        update_install_state: "still_unknown",
        update_objective_id: "objective-update-1",
      }),
    );

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.invoke).toHaveBeenCalledWith("reserve_update_install", {
      targetVersion: "1.79.1",
      targetBuild: TARGET_BUILD,
    });
    expect(mocks.install).not.toHaveBeenCalled();
    expect(mocks.relaunch).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "waiting_for_safe_restart",
      blockers: { update_install_state: "still_unknown" },
    });
  });

  it("fails closed when receipt admission is missing from an otherwise green snapshot", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "missing receipt admission",
      rawJson: { build_git_sha: TARGET_BUILD },
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);
    const { update_install_state: _omitted, ...greenWithoutReceipt } = idleSafety();
    mocks.invoke.mockResolvedValue(greenWithoutReceipt);

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.install).not.toHaveBeenCalled();
    expect(mocks.relaunch).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase.kind).toBe("waiting_for_safe_restart");
  });

  it("fails closed before download when canonical target build identity is missing", async () => {
    const update = {
      available: true,
      version: "1.79.1",
      body: "manifest without build proof",
      rawJson: {},
      download: mocks.download,
      install: mocks.install,
    };
    mocks.check.mockResolvedValue(update);

    await useUpdaterStore.getState().checkNow();
    await useUpdaterStore.getState().install();

    expect(mocks.download).not.toHaveBeenCalled();
    expect(mocks.invoke).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "waiting_for_safe_restart",
      safetyCheckError: expect.stringContaining("build_git_sha"),
    });
  });

  it("reconciles a post-restart target version and build before checking again", async () => {
    mocks.invoke.mockResolvedValue({
      state: "applied",
      objective_id: "objective-update-1",
      target_version: "1.79.1",
      target_build: "17901",
    });

    await useUpdaterStore.getState().initialize();

    expect(mocks.invoke).toHaveBeenCalledWith("observe_update_install");
    expect(useUpdaterStore.getState()).toMatchObject({
      currentVersion: "1.79.1",
      phase: { kind: "up_to_date" },
    });
  });

  it("still observes the durable install receipt when frontend version lookup fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    mocks.getVersion.mockRejectedValue(new Error("app API unavailable"));
    mocks.invoke.mockResolvedValue({
      state: "applied",
      objective_id: "objective-update-1",
      target_version: "1.79.1",
      target_build: "17901",
    });

    await useUpdaterStore.getState().initialize();

    expect(mocks.invoke).toHaveBeenCalledWith("observe_update_install");
    expect(useUpdaterStore.getState().currentVersion).toBe("1.79.1");
    expect(warn).toHaveBeenCalledWith("[updater] getVersion failed:", expect.any(Error));
    warn.mockRestore();
  });

  it("projects backend download progress instead of leaving a zero-blocker update queued", async () => {
    let progressHandler: ((event: { payload: Record<string, unknown> }) => void) | null = null;
    mocks.invoke.mockResolvedValue(null);
    mocks.listen.mockImplementation(async (_event, handler) => {
      progressHandler = handler;
      return () => undefined;
    });
    const update = {
      available: true,
      version: "1.79.1",
      body: "backend owned download",
      rawJson: { build_git_sha: TARGET_BUILD },
      download: mocks.download,
      install: mocks.install,
    } as unknown as Update;

    await useUpdaterStore.getState().initialize();
    expect(mocks.listen).toHaveBeenCalledWith(
      "update-install-progress",
      expect.any(Function),
    );
    useUpdaterStore.setState({
      phase: {
        kind: "waiting_for_safe_restart",
        update,
        blockers: idleSafety({
          safe_to_restart: false,
          restart_reserved: false,
          update_install_state: "queued",
        }),
        safetyCheckError: null,
        checkedAt: Date.now(),
      },
    });

    expect(progressHandler).not.toBeNull();
    const emitProgress = progressHandler as unknown as (event: {
      payload: Record<string, unknown>;
    }) => void;
    emitProgress({
      payload: {
        target_version: update.version,
        target_build: TARGET_BUILD,
        phase: "downloading",
        received: 8 * 1024 * 1024,
        total: 32 * 1024 * 1024,
      },
    });

    expect(useUpdaterStore.getState().phase).toMatchObject({
      kind: "downloading",
      received: 8 * 1024 * 1024,
      total: 32 * 1024 * 1024,
    });

    emitProgress({
      payload: {
        target_version: update.version,
        target_build: TARGET_BUILD,
        phase: "installing",
        received: 32 * 1024 * 1024,
        total: 32 * 1024 * 1024,
      },
    });
    emitProgress({
      payload: {
        target_version: update.version,
        target_build: TARGET_BUILD,
        phase: "downloading",
        received: 16 * 1024 * 1024,
        total: 32 * 1024 * 1024,
      },
    });

    expect(useUpdaterStore.getState().phase.kind).toBe("installing");
  });
});
