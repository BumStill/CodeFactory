// SPDX-License-Identifier: Apache-2.0
//
// P3 tool-policy: the 「工具门控建议」 section scans for flaky, currently-allowed
// tools (propose_tool_gates) and gates one only when the human clicks
// (apply_tool_gate) — verifying the read-only-scan / human-gated-enable contract.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("../../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

import { ToolGateSection } from "./ProfilePage";

describe("ToolGateSection — flaky-tool gating (P3 tool-policy)", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
  });

  it("scans read-only, then gates a tool only on click", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "propose_tool_gates") {
        return Promise.resolve([
          { tool: "edit_file", total: 10, errors: 4, rate: 40, observation: "o" },
        ]);
      }
      return Promise.resolve(undefined); // apply_tool_gate
    });

    render(<ToolGateSection />);

    // Starts unscanned — nothing has been read or changed.
    expect(screen.getByText("还没有扫描")).toBeInTheDocument();
    expect(mocks.invoke).not.toHaveBeenCalled();

    // Scan → read-only propose_tool_gates surfaces the flaky tool.
    fireEvent.click(screen.getByText("扫描易错工具"));
    await waitFor(() => expect(screen.getByText("edit_file")).toBeInTheDocument());
    expect(mocks.invoke).toHaveBeenCalledWith("propose_tool_gates");
    expect(screen.getByText(/40%/)).toBeInTheDocument();
    // The scan mutated nothing — no gate has been applied.
    expect(mocks.invoke).not.toHaveBeenCalledWith("apply_tool_gate", expect.anything());

    // Click 启用门控 → the human gate fires apply_tool_gate for that tool only.
    fireEvent.click(screen.getByText("启用门控"));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("apply_tool_gate", { tool: "edit_file" }),
    );

    // Proposal cleared + the gate is confirmed back to the user.
    await waitFor(() =>
      expect(screen.queryByText("启用门控")).not.toBeInTheDocument(),
    );
    expect(screen.getByText(/已门控/)).toBeInTheDocument();
  });
});
