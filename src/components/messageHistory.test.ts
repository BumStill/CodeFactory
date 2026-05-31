// SPDX-License-Identifier: Apache-2.0
import { describe, it, expect } from "vitest";
import { recallHistory, pushHistory } from "./messageHistory";

describe("recallHistory", () => {
  const stack = ["first", "second", "third"]; // oldest..newest

  it("Up from the draft walks newest → oldest, clamping at the oldest", () => {
    expect(recallHistory(stack, 0, "up", "draft")).toEqual({ value: "third", pos: 1 });
    expect(recallHistory(stack, 1, "up", "draft")).toEqual({ value: "second", pos: 2 });
    expect(recallHistory(stack, 2, "up", "draft")).toEqual({ value: "first", pos: 3 });
    expect(recallHistory(stack, 3, "up", "draft")).toEqual({ value: "first", pos: 3 });
  });

  it("Down walks back toward the draft, clamping at the draft", () => {
    expect(recallHistory(stack, 3, "down", "draft")).toEqual({ value: "second", pos: 2 });
    expect(recallHistory(stack, 1, "down", "draft")).toEqual({ value: "draft", pos: 0 });
    expect(recallHistory(stack, 0, "down", "draft")).toEqual({ value: "draft", pos: 0 });
  });

  it("an empty history stays on the draft", () => {
    expect(recallHistory([], 0, "up", "d")).toEqual({ value: "d", pos: 0 });
  });
});

describe("pushHistory", () => {
  it("appends, collapses an immediate duplicate, and ignores empty", () => {
    expect(pushHistory([], "a")).toEqual(["a"]);
    expect(pushHistory(["a"], "b")).toEqual(["a", "b"]);
    expect(pushHistory(["a", "b"], "b")).toEqual(["a", "b"]); // dup collapsed
    expect(pushHistory(["a"], "")).toEqual(["a"]); // empty ignored
  });
});
