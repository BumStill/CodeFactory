// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ResourcesPage } from "./ResourcesPage";

const mocks = vi.hoisted(() => ({
  loadLibraries: vi.fn(),
  registerLibrary: vi.fn(),
  scanLibrary: vi.fn(),
  setLibraryEnabled: vi.fn(),
  deleteLibrary: vi.fn(),
  open: vi.fn(),
  loadSkills: vi.fn(),
}));

const knowledgeState = vi.hoisted(() => ({
  libraries: [{
    id: "kb-1",
    name: "产品资料",
    root_path: "/Users/x/Knowledge",
    enabled: true,
    created_at: "2026-07-16T00:00:00Z",
    last_scan_at: "2026-07-16T01:00:00Z",
    scan_status: "completed",
  }],
  scanSummaries: {
    "kb-1": {
      library_id: "kb-1",
      scanned_files: 4,
      indexed_documents: 3,
      failed_documents: 1,
      chunks_indexed: 22,
    },
  },
  loading: false,
  scanning: {} as Record<string, boolean>,
  error: null as string | null,
  loadLibraries: mocks.loadLibraries,
  registerLibrary: mocks.registerLibrary,
  scanLibrary: mocks.scanLibrary,
  setLibraryEnabled: mocks.setLibraryEnabled,
  deleteLibrary: mocks.deleteLibrary,
}));

vi.mock("../../stores/knowledge", () => ({
  useKnowledgeStore: () => knowledgeState,
}));

vi.mock("../../stores/skills", () => ({
  useSkillsStore: () => ({
    skills: [],
    loading: false,
    catalogError: null,
    loadSkills: mocks.loadSkills,
    enableSkill: vi.fn(),
    disableSkill: vi.fn(),
    installFromUrl: vi.fn(),
    installMarketplace: vi.fn(),
    selectSourceDirectory: vi.fn(),
    importFromDirectory: vi.fn(),
    createSkill: vi.fn(),
    updateSkill: vi.fn(),
    deleteSkill: vi.fn(),
    getSkillDetail: vi.fn(),
  }),
}));

vi.mock("../../stores/chat", () => ({
  useChatStore: (selector: (state: { activeSession: null }) => unknown) => selector({ activeSession: null }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("ResourcesPage", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockReset());
    mocks.loadLibraries.mockResolvedValue(undefined);
    mocks.registerLibrary.mockResolvedValue(undefined);
    mocks.scanLibrary.mockResolvedValue(undefined);
    mocks.setLibraryEnabled.mockResolvedValue(undefined);
    mocks.deleteLibrary.mockResolvedValue(undefined);
    mocks.loadSkills.mockResolvedValue(undefined);
    mocks.open.mockResolvedValue("/Users/x/NewKnowledge");
    vi.spyOn(window, "confirm").mockReturnValue(true);
  });

  it("manages knowledge libraries from the backend page", async () => {
    const user = userEvent.setup();
    render(<ResourcesPage onBack={() => {}} />);

    expect(await screen.findByRole("heading", { name: "资源中心" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "知识库" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "技能" })).toBeInTheDocument();
    expect(screen.getByText("产品资料")).toBeInTheDocument();
    expect(screen.getByText(/3 文档 · 22 片段 · 1 失败/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "扫描知识库 产品资料" }));
    expect(mocks.scanLibrary).toHaveBeenCalledWith("kb-1");

    await user.click(screen.getByRole("button", { name: "禁用知识库 产品资料" }));
    expect(mocks.setLibraryEnabled).toHaveBeenCalledWith("kb-1", false);

    await user.click(screen.getByRole("button", { name: "删除知识库 产品资料" }));
    expect(window.confirm).toHaveBeenCalled();
    expect(mocks.deleteLibrary).toHaveBeenCalledWith("kb-1");
  });

  it("adds a local folder and exposes the skills manager as a backend tab", async () => {
    const user = userEvent.setup();
    render(<ResourcesPage onBack={() => {}} />);

    await user.click(screen.getByRole("button", { name: "添加知识库" }));
    await waitFor(() => expect(mocks.registerLibrary).toHaveBeenCalledWith("NewKnowledge", "/Users/x/NewKnowledge"));

    await user.click(screen.getByRole("button", { name: "技能" }));
    expect(await screen.findByText("已安装")).toBeInTheDocument();
    expect(screen.getByText("市场")).toBeInTheDocument();
  });
});
