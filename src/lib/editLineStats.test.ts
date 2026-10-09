// SPDX-License-Identifier: Apache-2.0
//
// M36: the collapsed edit row used to print the *character* count of
// `old_string`/`new_string` ("985b → 4133b"), which tells the user nothing.
// Git, GitHub and every editor answer the question with "+X −Y" *lines*.
// These tests pin the counting rules, including the line-ending edges that
// silently inflate a naive `split("\n")` implementation.

import { describe, it, expect } from "vitest";
import { countLineChanges, lineStatsText } from "./editLineStats";

describe("countLineChanges", () => {
  it("counts a one-line replacement as +1 −1", () => {
    expect(countLineChanges("a\nb\nc", "a\nB\nc")).toEqual({ added: 1, removed: 1 });
  });

  it("counts 3 lines added after context as +3 −0", () => {
    expect(countLineChanges("a\nb", "a\nb\nc\nd\ne")).toEqual({ added: 3, removed: 0 });
  });

  it("counts a deleted block as +0 −N", () => {
    expect(countLineChanges("a\nb\nc\nd", "a\nd")).toEqual({ added: 0, removed: 2 });
  });

  it("reports +0 −0 for identical text", () => {
    expect(countLineChanges("same\ntext", "same\ntext")).toEqual({ added: 0, removed: 0 });
  });

  it("does not count unchanged context lines as added and removed", () => {
    const before = Array.from({ length: 40 }, (_, i) => `line ${i}`).join("\n");
    const after = before.replace("line 20", "line twenty");
    expect(countLineChanges(before, after)).toEqual({ added: 1, removed: 1 });
  });

  it("treats a changed line ending as no line change", () => {
    expect(countLineChanges("a\r\nb\r\nc", "a\nb\nc")).toEqual({ added: 0, removed: 0 });
  });

  it("counts CRLF content identically to LF content", () => {
    expect(countLineChanges("a\r\nb\r\n", "a\r\nB\r\n")).toEqual({ added: 1, removed: 1 });
  });

  it("does not count a trailing newline as an added line", () => {
    expect(countLineChanges("a\nb", "a\nb\n")).toEqual({ added: 0, removed: 0 });
  });

  it("counts content inserted into an empty file as added lines", () => {
    expect(countLineChanges("", "x\ny\n")).toEqual({ added: 2, removed: 0 });
  });

  it("counts clearing a file as removed lines", () => {
    expect(countLineChanges("x\ny", "")).toEqual({ added: 0, removed: 2 });
  });

  it("handles a pure append at the end of the file", () => {
    expect(countLineChanges("a\nb\nc", "a\nb\nc\nd\ne")).toEqual({ added: 2, removed: 0 });
  });
});

describe("lineStatsText", () => {
  it("renders git-style +X −Y", () => {
    expect(lineStatsText({ added: 3, removed: 1 })).toBe("+3 −1");
  });
});
