// SPDX-License-Identifier: Apache-2.0
//
// The permission dialog's "完全访问并允许" button must actually GRANT full
// access — persist it so the running turn AND future sessions stop prompting —
// not silently behave like "仅允许一次". For several releases the two buttons
// were wired to the identical `respondPermission(true)` no-op, so the
// full-access button lied. This pins the real contract.

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
        pendingPermission: { toolCallId: "tc1", toolName: "write_file", args: {} },
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

describe("respondPermission — full access", () => {
  it("persists full_access, then responds allow for the current call", async () => {
    await useChatStore.getState().respondPermission(true, { grantFullAccess: true });

    const saveCall = invokeMock.mock.calls.find((c) => c[0] === "save_settings");
    expect(saveCall, "save_settings must be invoked").toBeTruthy();
    expect((saveCall![1] as { newSettings: Settings }).newSettings.permissions.full_access).toBe(
      true,
    );
    // Shared settings now reflect full access — the running turn re-reads this.
    expect(useSettingsStore.getState().settings?.permissions.full_access).toBe(true);
    // The current call is still allowed.
    const respondCall = invokeMock.mock.calls.find((c) => c[0] === "respond_to_permission");
    expect(respondCall![1]).toEqual({ toolCallId: "tc1", allow: true });
  });

  it("allow-once never touches settings", async () => {
    await useChatStore.getState().respondPermission(true);
    expect(invokeMock.mock.calls.some((c) => c[0] === "save_settings")).toBe(false);
    const respondCall = invokeMock.mock.calls.find((c) => c[0] === "respond_to_permission");
    expect(respondCall![1]).toEqual({ toolCallId: "tc1", allow: true });
  });

  it("does not re-persist when full access is already enabled", async () => {
    useSettingsStore.setState({ settings: mkSettings(true) });
    seedPending();
    await useChatStore.getState().respondPermission(true, { grantFullAccess: true });
    expect(invokeMock.mock.calls.some((c) => c[0] === "save_settings")).toBe(false);
    const respondCall = invokeMock.mock.calls.find((c) => c[0] === "respond_to_permission");
    expect(respondCall![1]).toEqual({ toolCallId: "tc1", allow: true });
  });
});
