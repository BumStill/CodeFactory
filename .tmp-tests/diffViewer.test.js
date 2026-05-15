function assertEqual(actual, expected, label) {
    if (actual !== expected) {
        throw new Error(`${label}: expected ${String(expected)}, got ${String(actual)}`);
    }
}
function assertTruthy(value, label) {
    if (!value) {
        throw new Error(`${label}: expected truthy value`);
    }
}
const { parseUnifiedDiffResult } = await import("../components/DiffViewer.js");
const parsed = parseUnifiedDiffResult([
    "Edited D:\\CodeFactory\\notes.txt",
    "",
    "--- a/notes.txt",
    "+++ b/notes.txt",
    "@@ -1,2 +1,2 @@",
    " alpha",
    "-old",
    "+new",
].join("\n"));
assertEqual(parsed.summary, "Edited D:\\CodeFactory\\notes.txt", "summary before diff");
assertEqual(parsed.files.length, 1, "file count");
assertEqual(parsed.files[0]?.oldPath, "a/notes.txt", "old path");
assertEqual(parsed.files[0]?.newPath, "b/notes.txt", "new path");
const lines = parsed.files[0]?.lines;
assertTruthy(lines, "parsed lines");
assertEqual(lines.length, 4, "diff line count");
assertEqual(lines[0]?.kind, "hunk", "hunk line kind");
assertEqual(lines[1]?.kind, "context", "context line kind");
assertEqual(lines[2]?.kind, "removed", "removed line kind");
assertEqual(lines[3]?.kind, "added", "added line kind");
export {};
