// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { FileArtifactCard, isDocumentPath, resolveFilePath } from "./FileArtifactCard";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));

describe("FileArtifactCard", () => {
  it("recognizes explicit project document paths but not ordinary inline code", () => {
    expect(isDocumentPath("docs/plan.md")).toBe(true);
    expect(isDocumentPath("src/App.tsx")).toBe(true);
    expect(isDocumentPath("foo.ts")).toBe(false);
    expect(isDocumentPath("https://example.com/a.md")).toBe(false);
  });

  it("resolves relative paths against the active session cwd", () => {
    expect(resolveFilePath("/project", "docs/plan.md")).toBe("/project/docs/plan.md");
    expect(resolveFilePath("C:\\project", "docs\\plan.md")).toBe("C:\\project\\docs\\plan.md");
  });

  it("renders friendly actions and a compact filename", () => {
    render(<FileArtifactCard path="docs/plans/roadmap.md" cwd="/project" onPreview={vi.fn()} />);
    expect(screen.getByText("roadmap.md")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "查看文档 roadmap.md" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "系统打开 roadmap.md" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制路径 roadmap.md" })).toBeInTheDocument();
  });
});
