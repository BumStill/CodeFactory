// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { parseUnifiedDiffResult } from "../components/DiffViewer";

describe("diff viewer unified diff parsing", () => {
  it("splits the summary from the diff and classifies each line", () => {
    const parsed = parseUnifiedDiffResult(
      [
        "Edited D:\\CodeFactory\\notes.txt",
        "",
        "--- a/notes.txt",
        "+++ b/notes.txt",
        "@@ -1,2 +1,2 @@",
        " alpha",
        "-old",
        "+new",
      ].join("\n"),
    );

    expect(parsed.summary).toBe("Edited D:\\CodeFactory\\notes.txt");
    expect(parsed.files).toHaveLength(1);
    expect(parsed.files[0]?.oldPath).toBe("a/notes.txt");
    expect(parsed.files[0]?.newPath).toBe("b/notes.txt");

    const lines = parsed.files[0]?.lines;
    expect(lines).toBeTruthy();
    expect(lines).toHaveLength(4);
    expect(lines?.[0]?.kind).toBe("hunk");
    expect(lines?.[1]?.kind).toBe("context");
    expect(lines?.[2]?.kind).toBe("removed");
    expect(lines?.[3]?.kind).toBe("added");
  });
});
