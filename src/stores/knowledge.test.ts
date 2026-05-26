// SPDX-License-Identifier: Apache-2.0
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
}));

vi.mock("../lib/tauri", async (orig) => {
  const real = (await orig()) as Record<string, unknown>;
  return { ...real, invoke: mocks.invoke };
});

import { useKnowledgeStore } from "./knowledge";

const library = {
  id: "kb-1",
  name: "历史方案库",
  root_path: "/Users/x/Knowledge",
  enabled: true,
  created_at: "2026-05-26T00:00:00Z",
  last_scan_at: null,
  scan_status: "idle",
};

describe("knowledge store", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    useKnowledgeStore.setState({
      libraries: [],
      scanSummaries: {},
      loading: false,
      scanning: {},
      error: null,
    });
  });

  it("loads registered libraries", async () => {
    mocks.invoke.mockResolvedValue([library]);

    await useKnowledgeStore.getState().loadLibraries();

    expect(mocks.invoke).toHaveBeenCalledWith("list_knowledge_libraries");
    expect(useKnowledgeStore.getState().libraries).toEqual([library]);
    expect(useKnowledgeStore.getState().error).toBeNull();
  });

  it("registers a folder using the backend request shape", async () => {
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "register_knowledge_library") return Promise.resolve(library);
      if (cmd === "list_knowledge_libraries") return Promise.resolve([library]);
      return Promise.resolve(undefined);
    });

    await useKnowledgeStore.getState().registerLibrary("历史方案库", "/Users/x/Knowledge");

    expect(mocks.invoke).toHaveBeenCalledWith("register_knowledge_library", {
      request: { name: "历史方案库", root_path: "/Users/x/Knowledge" },
    });
    expect(useKnowledgeStore.getState().libraries).toEqual([library]);
  });

  it("records scan summaries and clears per-library scanning state", async () => {
    const summary = {
      library_id: "kb-1",
      scanned_files: 3,
      indexed_documents: 2,
      failed_documents: 1,
      chunks_indexed: 16,
    };
    mocks.invoke.mockImplementation((cmd: string) => {
      if (cmd === "scan_knowledge_library") return Promise.resolve(summary);
      if (cmd === "list_knowledge_libraries") return Promise.resolve([library]);
      return Promise.resolve(undefined);
    });

    await useKnowledgeStore.getState().scanLibrary("kb-1");

    expect(mocks.invoke).toHaveBeenCalledWith("scan_knowledge_library", {
      libraryId: "kb-1",
    });
    expect(useKnowledgeStore.getState().scanSummaries["kb-1"]).toEqual(summary);
    expect(useKnowledgeStore.getState().scanning["kb-1"]).toBe(false);
  });

  it("keeps existing libraries visible when loading fails", async () => {
    useKnowledgeStore.setState({ libraries: [library] });
    mocks.invoke.mockRejectedValue(new Error("db locked"));

    await useKnowledgeStore.getState().loadLibraries();

    expect(useKnowledgeStore.getState().libraries).toEqual([library]);
    expect(useKnowledgeStore.getState().error).toContain("db locked");
  });
});
