// SPDX-License-Identifier: Apache-2.0
//
// The permission dialog's "信任本会话并允许" button must grant trust to the
// CURRENT session only. Permissions are no longer a global tool-list settings
// page; this pins that the button persists sessions.permission_mode='trusted'
// before allowing the current call.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore, freshRuntime } from "./chat";
import { useSettingsStore } from "./settings";
import type { Settings } from "../lib/tauri";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({
  invoke: invokeMock,
  onStream: vi.fn(async () => () => {}),
  onSessionUpdated: vi.fn(async () => () => {}),
  sendMessageAnonymous: vi.fn(async () => {}),
}));

// Persisting settings applies the theme, which touches the native app API.
vi.mock("@tauri-apps/api/app", () => ({ setTheme: vi.fn(async () => {}) }));

function mkSettings(fullAccess: boolean): Settings {
  return {
    endpoints: {},
    default_endpoint: "",
    default_model: "",
    permissions: { allow: [], ask: [], deny: [], full_access: fullAccess },
    shell: { shell: "bash" },
    auto_create_pr: false,
    theme: "dark",
    font_family: "inter",
    mono_font_family: "jetbrains-mono",
    font_size: 14,
  } as Settings;
}

const session = {
  id: "A",
  title: "A",
  cwd: "/p/A",
  model_id: "m",
  created_at: 0,
  updated_at: 0,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};

function seedPending() {
  useChatStore.setState({
    activeSession: session as never,
    runtime: {
      A: {
        ...freshRuntime(),
        pendingPermission: { intentId: "intent-tc1", toolCallId: "tc1", toolName: "write_file", args: {} },
      },
    },
  });
}

beforeEach(() => {
  invokeMock.mockReset();
  // `save_settings` echoes back the settings it was handed; everything else
  // (respond_to_permission, …) resolves to undefined.
  invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "save_settings") return (args as { newSettings: Settings }).newSettings;
    return undefined;
  });
  useSettingsStore.setState({ settings: mkSettings(false) });
  seedPending();
});

describe("respondPermission — session permission mode", () => {
  it("persists trusted mode on the current session, then responds allow", async () => {
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "update_session_permission_mode") {
        return { ...session, permission_mode: (args as { mode: string }).mode };
      }
      return undefined;
    });

    await useChatStore.getState().respondPermission(true, { grantFullAccess: true });

    const modeCall = invokeMock.mock.calls.find((c) => c[0] === "update_session_permission_mode");
    expect(modeCall![1]).toEqual({ sessionId: "A", mode: "trusted" });
    expect(useChatStore.getState().activeSession?.permission_mode).toBe("trusted");
    expect(invokeMock.mock.calls.some((c) => c[0] === "save_settings")).toBe(false);
    const respondCall = invokeMock.mock.calls.find((c) => c[0] === "respond_to_permission");
    expect(respondCall![1]).toEqual({ intentId: "intent-tc1", allow: true });
  });

  it("allow-once never touches settings", async () => {
    await useChatStore.getState().respondPermission(true);
    expect(invokeMock.mock.calls.some((c) => c[0] === "save_settings")).toBe(false);
    const respondCall = invokeMock.mock.calls.find((c) => c[0] === "respond_to_permission");
    expect(respondCall![1]).toEqual({ intentId: "intent-tc1", allow: true });
  });

  it("falls back to allow-once (not silently) when persisting trusted mode fails", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "update_session_permission_mode") throw new Error("db full");
      return undefined;
    });

    await useChatStore.getState().respondPermission(true, { grantFullAccess: true });

    expect(consoleError).toHaveBeenCalled();
    const respondCall = invokeMock.mock.calls.find((c) => c[0] === "respond_to_permission");
    expect(respondCall![1]).toEqual({ intentId: "intent-tc1", allow: true });
    consoleError.mockRestore();
  });
});
