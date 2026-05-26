// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("../lib/tauri", () => ({
  invoke: invokeMock,
}));

import { EvidenceViewer, type EvidencePack } from "./EvidenceViewer";

const pack: EvidencePack = {
  manifest: {
    spec_req_id: "CF-KB-R2",
    spec_title: "本地文件参考来源可见化",
    task_run_ids: ["task-1"],
    session_id: "session-1",
    created_at: "2026-05-26T08:00:00Z",
    completed_at: "2026-05-26T08:01:00Z",
    status: "passed",
    total_tasks: 1,
    completed_tasks: 1,
    failed_tasks: 0,
    total_tool_calls: 1,
    files_changed: 0,
    verification_passed: true,
    total_tokens: 1000,
    duration_minutes: 1,
    path: "/tmp/evidence",
  },
  summary_md: "# Summary",
  tool_calls: [],
  knowledge_refs: [
    {
      id: "retrieval-1",
      session_id: "session-1",
      task_id: "task-1",
      query: "产品路线图",
      filters: { library_ids: ["kb-1"], top_k: 3 },
      result_refs: [
        {
          chunk_id: "chunk-1",
          document_id: "doc-1",
          path: "/Users/leo/Knowledge/roadmap.pptx",
          page: null,
          slide: 4,
        },
      ],
      created_at: "2026-05-26T08:00:10Z",
      latency_ms: 42,
    },
  ],
  files_changed: [],
  verification: [],
  git_commits: [],
  ai_collaboration: {
    model: "test-model",
    total_tokens: 1000,
    assumptions: [],
    review_points: [],
  },
};

describe("EvidenceViewer knowledge refs", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("shows retrieved local file refs in a Sources tab", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_evidence_pack") return Promise.resolve(pack);
      return Promise.resolve(undefined);
    });

    render(<EvidenceViewer packPath="/tmp/evidence" onClose={() => {}} />);

    await waitFor(() => expect(screen.getByText("CF-KB-R2")).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: /Sources/ }));

    expect(screen.getByText("产品路线图")).toBeInTheDocument();
    expect(screen.getByText("roadmap.pptx")).toBeInTheDocument();
    expect(screen.getByText("slide 4")).toBeInTheDocument();
    expect(screen.getByText("chunk-1")).toBeInTheDocument();
    expect(screen.getByText("42ms")).toBeInTheDocument();
  });
});
