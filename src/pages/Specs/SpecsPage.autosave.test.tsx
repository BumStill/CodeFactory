// SPDX-License-Identifier: Apache-2.0
//
// Debounced spec auto-save used to `.catch(() => {})`, so a failed write was
// dropped silently and the user kept editing a spec they believed was saved —
// a data-loss-class surprise on the spec→task primary path. The editor must
// surface the failure and offer a retry. This pins that contract.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@testing-library/react";
import { SpecsPage } from "./SpecsPage";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  saveSpec: vi.fn(),
  loadSpecs: vi.fn(async () => {}),
  updateActiveContent: vi.fn(),
}));

const activeSpec = {
  meta: {
    req_id: "REQ-1",
    title: "Spec One",
    status: "draft",
    created_at: "",
    updated_at: "",
    tags: [],
    acceptance_criteria: [],
    file_path: "/proj/spec.md",
    rel_path: "spec.md",
  },
  content: "draft body",
  body: "draft body",
};

vi.mock("../../lib/tauri", () => ({ invoke: mocks.invoke }));

vi.mock("../../stores/chat", () => ({
  useChatStore: () => ({ activeSession: { id: "s1", cwd: "/proj", kind: "project" } }),
}));

vi.mock("../../stores/specs", () => ({
  useSpecsStore: () => ({
    specs: [activeSpec.meta],
    activeSpec,
    loading: false,
    loadSpecs: mocks.loadSpecs,
    openSpec: vi.fn(),
    saveSpec: mocks.saveSpec,
    deleteSpec: vi.fn(),
    approveSpec: vi.fn(),
    updateActiveContent: mocks.updateActiveContent,
  }),
}));

beforeEach(() => {
  mocks.invoke.mockReset().mockResolvedValue([]); // list_evidence_packs → []
  mocks.saveSpec.mockReset();
  mocks.loadSpecs.mockClear();
  mocks.updateActiveContent.mockClear();
});

afterEach(() => cleanup());

describe("SpecsPage auto-save error surfacing", () => {
  it("shows a failure banner when auto-save rejects, then clears it on a successful retry", async () => {
    mocks.saveSpec.mockRejectedValueOnce(new Error("disk full"));
    render(<SpecsPage onBack={() => {}} onOpenWorkspace={() => {}} />);

    // Edit the spec — the debounced save (1000ms) fires and rejects.
    fireEvent.change(screen.getByDisplayValue("draft body"), {
      target: { value: "draft body edited" },
    });
    await waitFor(
      () => expect(mocks.saveSpec).toHaveBeenCalledWith("/proj/spec.md", "draft body edited"),
      { timeout: 2000 },
    );
    expect(await screen.findByText(/自动保存失败：Error: disk full/)).toBeInTheDocument();

    // Retry succeeds → the banner disappears.
    mocks.saveSpec.mockResolvedValueOnce(activeSpec.meta);
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(screen.queryByText(/自动保存失败/)).toBeNull());
  });

  it("keeps silent when auto-save succeeds", async () => {
    mocks.saveSpec.mockResolvedValue(activeSpec.meta);
    render(<SpecsPage onBack={() => {}} onOpenWorkspace={() => {}} />);

    fireEvent.change(screen.getByDisplayValue("draft body"), {
      target: { value: "draft body ok" },
    });
    await waitFor(
      () => expect(mocks.saveSpec).toHaveBeenCalledWith("/proj/spec.md", "draft body ok"),
      { timeout: 2000 },
    );
    expect(screen.queryByText(/自动保存失败/)).toBeNull();
  });
});
