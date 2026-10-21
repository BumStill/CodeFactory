// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect, vi } from "vitest";
import { openFileLink, resolveLocalPath } from "../components/MessageFileLinks";
vi.mock("../lib/tauri", () => ({ invoke: vi.fn().mockResolvedValue(null) }));
import { invoke } from "../lib/tauri";

describe("M135 local file links", () => {
  it.each([
    ["/project", "docs/a.md", "/project/docs/a.md"],
    ["C:\\work", "docs/a.md", "C:\\work\\docs\\a.md"],
    [null, "file:///C:/Users/A%20B/report.pdf", "C:/Users/A B/report.pdf"],
    [null, "file://server/share/a.pdf", "//server/share/a.pdf"],
    [null, "\\\\?\\C:\\Users\\report.pdf", "C:\\Users\\report.pdf"],
    [null, "\\\\?\\UNC\\server\\share\\a.pdf", "\\\\server\\share\\a.pdf"],
  ])("resolves %s + %s", (cwd, path, expected) => {
    expect(resolveLocalPath(cwd, path)).toBe(expected);
  });
  it("routes a file link through the controlled native command", async () => {
    await openFileLink("docs/a.md", "/project", "synthetic-session");
    expect(invoke).toHaveBeenCalledWith("open_chat_link", { target: "docs/a.md", cwd: "/project", sessionId: "synthetic-session" });
  });
});
