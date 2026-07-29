// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { PermissionDialog } from "./PermissionDialog";
import type { PendingPermission } from "../stores/chat";

function baseRequest(overrides: Partial<PendingPermission>): PendingPermission {
  return {
    toolCallId: "tc-1",
    toolName: "bash",
    args: {},
    ...overrides,
  };
}

const noop = { onAllow: vi.fn(), onDeny: vi.fn(), onAllowFullAccess: vi.fn() };

describe("PermissionDialog tool-args preview", () => {
  it("renders a real diff for edit_file instead of raw JSON", () => {
    render(
      <PermissionDialog
        request={baseRequest({
          toolName: "edit_file",
          args: { path: "src/app.ts", old_string: "old line", new_string: "new line" },
        })}
        trusted={false}
        {...noop}
      />,
    );

    expect(screen.getByText(/old line/)).toBeInTheDocument();
    expect(screen.getByText(/new line/)).toBeInTheDocument();
    // The raw JSON blob (quoted key names) must not appear.
    expect(screen.queryByText(/"old_string"/)).not.toBeInTheDocument();
  });

  it("renders a labeled content preview for write_file, not raw JSON", () => {
    render(
      <PermissionDialog
        request={baseRequest({
          toolName: "write_file",
          args: { path: "src/new.ts", content: "export const x = 1;" },
        })}
        trusted={false}
        {...noop}
      />,
    );

    expect(screen.getByText("src/new.ts")).toBeInTheDocument();
    expect(screen.getByText(/export const x = 1;/)).toBeInTheDocument();
    expect(screen.queryByText(/"content"/)).not.toBeInTheDocument();
  });

  it("offers to trust only the current session", () => {
    render(
      <PermissionDialog
        request={baseRequest({ toolName: "bash", args: { command: "pnpm test" } })}
        trusted={false}
        {...noop}
      />,
    );

    expect(screen.getByRole("button", { name: "信任本会话并允许" })).toBeInTheDocument();
    expect(screen.queryByText("完全访问并允许")).toBeNull();
  });

  it("falls back to raw JSON for tools without a structured preview", () => {
    render(
      <PermissionDialog
        request={baseRequest({ toolName: "bash", args: { command: "ls -la" } })}
        trusted={false}
        {...noop}
      />,
    );

    expect(screen.getByText(/"command"/)).toBeInTheDocument();
    expect(screen.getByText(/ls -la/)).toBeInTheDocument();
  });
});
