// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PermissionDialog } from "./PermissionDialog";
import type { PendingPermission } from "../stores/chat";

function baseRequest(overrides: Partial<PendingPermission>): PendingPermission {
  return {
    intentId: "intent-test",
    toolCallId: "tc-1",
    toolName: "bash",
    args: {},
    ...overrides,
  };
}

const noop = { onAllow: vi.fn(), onDeny: vi.fn(), onAllowFullAccess: vi.fn() };

describe("PermissionDialog tool-args preview", () => {
  it("is an accessible modal and explains that expiry is not a user denial", () => {
    render(
      <PermissionDialog
        request={baseRequest({
          expiresAt: Date.now() + 60_000,
        })}
        trusted={false}
        {...noop}
      />,
    );

    expect(screen.getByRole("dialog", { name: "需要权限" })).toBeInTheDocument();
    expect(screen.getByText(/超时.*不会记成你拒绝/)).toBeInTheDocument();
  });

  it("focuses the safe refusal action first and traps keyboard focus inside the modal", async () => {
    render(
      <PermissionDialog
        request={baseRequest({ toolName: "bash", args: { command: "pnpm test" } })}
        trusted={false}
        {...noop}
      />,
    );

    const deny = screen.getByRole("button", { name: "拒绝" });
    const trust = screen.getByRole("button", { name: "信任本会话并允许" });
    expect(deny).toHaveClass("min-h-11", "lg:min-h-9");
    expect(screen.getByRole("button", { name: "仅允许一次" })).toHaveClass("min-h-11", "lg:min-h-9");
    expect(trust).toHaveClass("min-h-11", "lg:min-h-9");
    await waitFor(() => expect(deny).toHaveFocus());

    await userEvent.tab({ shift: true });
    expect(trust).toHaveFocus();

    await userEvent.tab();
    expect(deny).toHaveFocus();
  });

  it("treats Escape as an explicit denial and restores focus to the opener", async () => {
    const onDeny = vi.fn();
    const onAllow = vi.fn();

    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button onClick={() => setOpen(true)}>运行受限工具</button>
          {open && (
            <PermissionDialog
              request={baseRequest({ toolName: "bash", args: { command: "pnpm test" } })}
              trusted={false}
              onAllow={onAllow}
              onDeny={() => {
                onDeny();
                setOpen(false);
              }}
              onAllowFullAccess={vi.fn()}
            />
          )}
        </>
      );
    }

    render(<Harness />);
    const opener = screen.getByRole("button", { name: "运行受限工具" });
    await userEvent.click(opener);
    await waitFor(() => expect(screen.getByRole("button", { name: "拒绝" })).toHaveFocus());

    await userEvent.keyboard("{Escape}");

    expect(onDeny).toHaveBeenCalledTimes(1);
    expect(onAllow).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: "需要权限" })).not.toBeInTheDocument();
    await waitFor(() => expect(opener).toHaveFocus());
  });

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
