// SPDX-License-Identifier: Apache-2.0
//
// Self-evolution P4 — the only user-facing surface is a READ-ONLY proposal.
// These tests pin the contract the UI must keep: it calls the global
// `self_improvement_proposal` command (no cwd), renders the returned markdown,
// and never implies it changed anything. The framing ("还没有生成提案",
// proposal-only) is part of the safety model, so it's asserted, not incidental.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("../../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

import { SelfImprovementSection } from "./ProfilePage";

describe("SelfImprovementSection — P4 read-only proposal surface", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
  });

  it("shows a read-only placeholder before anything is generated", () => {
    render(<SelfImprovementSection />);
    expect(screen.getByText("还没有生成提案")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /生成改进提案/ }),
    ).toBeInTheDocument();
    // No command fires until the user asks.
    expect(mocks.invoke).not.toHaveBeenCalled();
  });

  it("invokes the GLOBAL command (no cwd) and renders the returned markdown", async () => {
    mocks.invoke.mockResolvedValue(
      "# CodeFactory 自我改进提案\n\n## 工具可靠性\n\n- `bash` 在 6 次调用里失败 5 次\n",
    );
    render(<SelfImprovementSection />);

    fireEvent.click(screen.getByRole("button", { name: /生成改进提案/ }));

    // Global aggregation across all projects — invoked with no argument.
    expect(mocks.invoke).toHaveBeenCalledWith("self_improvement_proposal");

    // The markdown body actually renders (heading text from the proposal).
    await waitFor(() =>
      expect(screen.getByText("工具可靠性")).toBeInTheDocument(),
    );

    // Once a proposal is shown the action becomes a re-run, not a first-run.
    expect(
      screen.getByRole("button", { name: /重新生成/ }),
    ).toBeInTheDocument();
  });

  it("surfaces a backend error and stays on the placeholder (never half-renders)", async () => {
    mocks.invoke.mockRejectedValue("database is locked");
    render(<SelfImprovementSection />);

    fireEvent.click(screen.getByRole("button", { name: /生成改进提案/ }));

    await waitFor(() =>
      expect(screen.getByText(/database is locked/)).toBeInTheDocument(),
    );
    expect(screen.getByText("还没有生成提案")).toBeInTheDocument();
  });
});
