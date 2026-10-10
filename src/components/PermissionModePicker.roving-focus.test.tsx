// SPDX-License-Identifier: Apache-2.0
// Pins the roving-focus rule the Windows CI flake exposed: on a loaded runner a
// keypress can reach the menu before any item owns focus, and ArrowDown must
// still move off the checked mode instead of wrapping back to the first item.

import { describe, expect, it, beforeEach } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

import { PermissionModePicker } from "./PermissionModePicker";
import { freshRuntime, useChatStore } from "../stores/chat";
import type { Session } from "../lib/tauri";

const session: Session = {
  id: "permission-mode-roving-session",
  title: "roving focus",
  cwd: "/tmp/codefactory-roving-focus",
  endpoint_id: "openrouter",
  model_id: "test-model",
  model_policy: "prefer",
  permission_mode: "standard",
  created_at: 1,
  updated_at: 1,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};

describe("PermissionModePicker roving focus", () => {
  beforeEach(() => {
    useChatStore.setState({
      sessions: [session],
      activeSession: session,
      draftSession: null,
      runtime: { [session.id]: freshRuntime() },
      activeModel: session.model_id,
      updateActiveSessionPermissionMode: async () => {},
    });
  });

  it("moves off the checked mode when ArrowDown arrives before any item owns focus", () => {
    render(<PermissionModePicker onChangeForAcceptance={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /会话权限：标准/ }));

    const items = screen.getAllByRole("menuitemradio");
    expect(items).toHaveLength(3);
    const checked = items.find((item) => item.getAttribute("aria-checked") === "true");
    expect(checked).toBeDefined();

    // Reproduce the race: the menu is open but focus never landed on an item.
    (document.activeElement as HTMLElement | null)?.blur?.();
    expect(items.includes(document.activeElement as HTMLElement)).toBe(false);

    fireEvent.keyDown(screen.getByRole("menu"), { key: "ArrowDown" });

    const trusted = items[items.indexOf(checked!) + 1];
    expect(document.activeElement).toBe(trusted);
  });

  it("focuses the checked mode as soon as the menu opens", () => {
    render(<PermissionModePicker onChangeForAcceptance={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /会话权限：标准/ }));
    const checked = screen
      .getAllByRole("menuitemradio")
      .find((item) => item.getAttribute("aria-checked") === "true");
    expect(document.activeElement).toBe(checked);
  });
});
