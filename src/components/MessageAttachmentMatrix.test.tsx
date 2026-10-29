// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
vi.mock("../lib/tauri", () => ({ invoke: vi.fn().mockResolvedValue(null) }));
import { MessageFileArtifactCard } from "./MessageFileArtifactCard";
import { ImagePreview } from "./ImagePreview";

describe("M190 R5/R6 attachment preview matrix", () => {
  it.each(["png", "jpg", "gif", "webp", "heic", "pdf", "docx", "pptx", "xlsx", "csv", "txt", "md", "ts", "mp3", "mp4", "zip", "unknown"])("%s retains original name, type, auto size and open action", (ext) => {
    const name = `very-long-synthetic-original-filename.${ext}`;
    render(<MessageFileArtifactCard path={`/synthetic/uuid.${ext}`} name={name} sizeBytes={2 * 1024 * 1024} />);
    expect(screen.getByText(name)).toHaveAttribute("title", name);
    expect(screen.getByText(ext.toUpperCase())).toBeInTheDocument();
    expect(screen.getByText("2.0 MB")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: `系统打开 ${name}` })).toBeEnabled();
  });
  it("image failure is readable and full caption survives the lightbox", () => {
    render(<ImagePreview src="/missing.png" alt="synthetic.png" caption="synthetic.png · 2.0 MB" thumbnailClassName="h-20" />);
    fireEvent.click(screen.getByRole("button", { name: "放大查看 synthetic.png" }));
    expect(screen.getByText("synthetic.png · 2.0 MB", { selector: "figcaption" })).toHaveAttribute("title", "synthetic.png · 2.0 MB");
    fireEvent.error(screen.getAllByRole("img", { name: "synthetic.png" })[0]);
    expect(screen.getByText(/图片无法预览.*文件可能已被删除/)).toBeInTheDocument();
  });
});
