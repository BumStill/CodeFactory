// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ToolCallCard } from "./ToolCallCard";
import type { ToolCallState } from "../stores/chatEvents";

describe("ToolCallCard knowledge sources", () => {
  it("renders kb_search source refs as file, slide, and chunk cards", async () => {
    const tc: ToolCallState = {
      id: "tc-1",
      name: "kb_search",
      args: JSON.stringify({ query: "产品路线图", top_k: 3 }),
      result: JSON.stringify([
        {
          chunk_id: "chunk-1",
          document_id: "doc-1",
          library_id: "kb-1",
          path: "/Users/leo/Knowledge/roadmap.pptx",
          kind: "pptx",
          title: "Roadmap",
          chunk_index: 2,
          page: null,
          slide: 4,
          heading: "Q3 launch",
          snippet: "复用历史路线图中的发布节奏和里程碑。",
          score: 3,
        },
      ]),
      status: "done",
      isError: false,
    };

    render(<ToolCallCard tc={tc} />);
    await userEvent.click(screen.getByRole("button"));

    expect(screen.getByText("sources 1")).toBeInTheDocument();
    expect(screen.getByText("roadmap.pptx")).toBeInTheDocument();
    expect(screen.getByText("slide 4")).toBeInTheDocument();
    expect(screen.getByText("chunk-1")).toBeInTheDocument();
    expect(screen.getByText(/复用历史路线图/)).toBeInTheDocument();
  });
});
