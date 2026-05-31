// SPDX-License-Identifier: Apache-2.0
//
// Pure shell-style input-history navigation for the message box.
//
// `pos` is the number of steps back from the live draft:
//   pos === 0          → the draft (what the user was typing)
//   pos === 1          → the most recently sent message
//   pos === stack.len  → the oldest sent message
// `stack` is oldest-first (newest at the end), matching append order.

export interface Recall {
  value: string;
  pos: number;
}

export function recallHistory(
  stack: string[],
  pos: number,
  direction: "up" | "down",
  draft: string,
): Recall {
  const n = stack.length;
  const next =
    direction === "up" ? Math.min(pos + 1, n) : Math.max(pos - 1, 0);
  const value = next === 0 ? draft : stack[n - next];
  return { value, pos: next };
}

/** Append a sent message, collapsing an immediate duplicate (shell-like). */
export function pushHistory(stack: string[], text: string): string[] {
  if (!text) return stack;
  if (stack[stack.length - 1] === text) return stack;
  return [...stack, text];
}
