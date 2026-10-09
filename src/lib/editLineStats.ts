// SPDX-License-Identifier: Apache-2.0
//
// M36 — line-level change stats for edit/write tool calls.
//
// The collapsed conversation row for an edit used to print the *character*
// count of the arguments ("985b → 4133b"). The user's report: "这有啥用啊，
// 肯定是+多少行-多少行，更符合常识吧". Git, GitHub and every editor answer
// that question with "+X −Y" lines, so this module owns the counting rules in
// one place, shared by the tool-call row and by the turn-wide total.

export interface LineChangeStats {
  added: number;
  removed: number;
}

/** Whether a tool call produces "lines changed" statistics, and how. */
export type ToolLineStatsKind = "edit" | "write";

export interface ToolLineStats extends LineChangeStats {
  kind: ToolLineStatsKind;
  /**
   * For writes only: the tool reported that it created a file that did not
   * exist before, so the new line count is honest as `+N`.
   */
  newFile: boolean;
}

/**
 * Split text into lines the way git counts them: CRLF and a lone CR are
 * line boundaries, and a single trailing newline terminates the last line
 * instead of starting an empty one (so "a\nb\n" is two lines, not three).
 */
export function splitLines(text: string): string[] {
  if (!text) return [];
  const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const parts = normalized.split("\n");
  if (parts.length > 0 && parts[parts.length - 1] === "") parts.pop();
  return parts;
}

export function lineCount(text: string): number {
  return splitLines(text).length;
}

/**
 * Above this many DP cells the exact LCS is dropped for the coarse answer
 * (everything in the changed region counts as removed + added). Prefix and
 * suffix trimming happens first, so this only triggers on a genuine
 * wholesale rewrite of enormous text.
 */
const MAX_DIFF_CELLS = 4_000_000;

/** Longest common subsequence length of two line arrays (rolling 1-row DP). */
function lcsLength(a: string[], b: string[]): number {
  const prev = new Uint32Array(b.length + 1);
  const current = new Uint32Array(b.length + 1);
  for (let i = 1; i <= a.length; i += 1) {
    for (let j = 1; j <= b.length; j += 1) {
      current[j] = a[i - 1] === b[j - 1]
        ? prev[j - 1] + 1
        : Math.max(prev[j], current[j - 1]);
    }
    prev.set(current);
    current.fill(0);
  }
  return prev[b.length];
}

/**
 * Count added and removed lines between two texts, ignoring unchanged
 * context entirely — a single changed line in a 40-line file is `+1 −1`,
 * never `+40 −40`.
 */
export function countLineChanges(before: string, after: string): LineChangeStats {
  const a = splitLines(before);
  const b = splitLines(after);

  let prefix = 0;
  while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix += 1;

  let suffix = 0;
  while (
    suffix < a.length - prefix
    && suffix < b.length - prefix
    && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]
  ) {
    suffix += 1;
  }

  const oldMiddle = a.slice(prefix, a.length - suffix);
  const newMiddle = b.slice(prefix, b.length - suffix);
  if (oldMiddle.length === 0) return { added: newMiddle.length, removed: 0 };
  if (newMiddle.length === 0) return { added: 0, removed: oldMiddle.length };
  if (oldMiddle.length * newMiddle.length > MAX_DIFF_CELLS) {
    return { added: newMiddle.length, removed: oldMiddle.length };
  }

  const common = lcsLength(oldMiddle, newMiddle);
  return { added: newMiddle.length - common, removed: oldMiddle.length - common };
}

export function isEmptyLineChange(stats: LineChangeStats): boolean {
  return stats.added === 0 && stats.removed === 0;
}

/** `+3 −1` — the spelling every developer already reads. */
export function lineStatsText(stats: LineChangeStats): string {
  return `+${stats.added} −${stats.removed}`;
}

function parseArgs(raw: string): Record<string, unknown> | null {
  try {
    const value = JSON.parse(raw) as unknown;
    return value && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

interface TextPair {
  before: string;
  after: string;
}

/**
 * Every `before`/`after` text pair an edit call actually changed. A tool that
 * batches several edits in one call (`edits: [...]`) contributes each one, so
 * the row and the turn total sum them instead of reporting only the first.
 */
function editTextPairs(args: Record<string, unknown>): TextPair[] {
  const pairs: TextPair[] = [];
  const push = (before: unknown, after: unknown) => {
    if (typeof before === "string" && typeof after === "string") {
      pairs.push({ before, after });
    } else if (typeof after === "string") {
      pairs.push({ before: "", after });
    }
  };

  if (Array.isArray(args.edits)) {
    for (const entry of args.edits) {
      if (!entry || typeof entry !== "object") continue;
      const edit = entry as Record<string, unknown>;
      push(edit.old_string ?? edit.old_text ?? edit.search, edit.new_string ?? edit.new_text ?? edit.replace);
    }
  }
  if (pairs.length === 0) {
    push(args.old_string ?? args.old_text, args.new_string ?? args.new_text);
  }
  return pairs;
}

/** A write result that reports creating a file that did not exist before. */
const NEW_FILE_HUNK = /@@ -0,0 \+\d+(?:,\d+)? @@/;
const NEW_FILE_WORDING = /\b(new file|created file|file created)\b|新(?:建|增)文件/i;

function writeContent(args: Record<string, unknown>): string | null {
  for (const key of ["content", "new_string", "text", "contents"]) {
    const value = args[key];
    if (typeof value === "string") return value;
  }
  return null;
}

/**
 * Line stats for a tool call, or null when the call is not a file change.
 * Write calls never invent a deletion count: the prior content is not in the
 * arguments, so an overwrite reports only the new line count, and `+N` is
 * used only when the result says the file was created.
 */
export function toolCallLineStats(
  name: string,
  raw: string,
  result: string | null | undefined,
): ToolLineStats | null {
  const args = parseArgs(raw);
  if (!args) return null;

  if (name === "edit_file" || name === "edit") {
    const pairs = editTextPairs(args);
    if (pairs.length === 0) return null;
    let added = 0;
    let removed = 0;
    for (const pair of pairs) {
      const stats = countLineChanges(pair.before, pair.after);
      added += stats.added;
      removed += stats.removed;
    }
    return { kind: "edit", added, removed, newFile: false };
  }

  if (name === "write_file" || name === "write") {
    const content = writeContent(args);
    if (content === null) return null;
    const text = result ?? "";
    const newFile = NEW_FILE_HUNK.test(text) || NEW_FILE_WORDING.test(text);
    return { kind: "write", added: lineCount(content), removed: 0, newFile };
  }

  return null;
}

/**
 * The plain-text form of a row's stats — used for the accessible name, where
 * a screen reader should hear the same numbers the eyes see.
 */
export function toolLineStatsLabel(stats: ToolLineStats): string | null {
  if (stats.kind === "edit") {
    return isEmptyLineChange(stats) ? null : lineStatsText(stats);
  }
  if (stats.added === 0) return null;
  return stats.newFile ? `+${stats.added}` : `写入 ${stats.added} 行`;
}
