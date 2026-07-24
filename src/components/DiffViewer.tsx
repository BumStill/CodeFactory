// SPDX-License-Identifier: Apache-2.0

export type DiffLineKind = "hunk" | "context" | "added" | "removed" | "meta";

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

export interface DiffFile {
  oldPath: string;
  newPath: string;
  lines: DiffLine[];
}

export interface ParsedDiffResult {
  summary: string;
  files: DiffFile[];
}

interface Props {
  output: string;
  parsed?: ParsedDiffResult;
}

export function DiffViewer({ output, parsed: parsedResult }: Props) {
  const parsed = parsedResult ?? parseUnifiedDiffResult(output);
  if (parsed.files.length === 0) return null;

  return (
    <div className="space-y-2 font-mono text-[11px]">
      {parsed.summary && (
        <div className="whitespace-pre-wrap break-words text-gray-400">{parsed.summary}</div>
      )}
      {parsed.files.map((file, fileIndex) => (
        <div key={`${file.oldPath}-${file.newPath}-${fileIndex}`} className="overflow-hidden rounded border border-border">
          <div className="flex items-center gap-2 border-b border-border bg-surface-3 px-2 py-1 text-gray-400">
            <span className="truncate text-red-700 dark:text-red-300">{file.oldPath}</span>
            <span className="text-gray-600">to</span>
            <span className="truncate text-green-700 dark:text-green-300">{file.newPath}</span>
          </div>
          <div className="max-h-80 overflow-auto bg-surface-1">
            {file.lines.map((line, lineIndex) => (
              <div
                key={`${lineIndex}-${line.text}`}
                className={`grid grid-cols-[1.5rem_minmax(0,1fr)] px-2 leading-5 ${classForLine(line.kind)}`}
              >
                <span className="select-none text-center text-gray-600">{prefixForLine(line.kind)}</span>
                <span className="whitespace-pre break-words">{line.text}</span>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

export function parseUnifiedDiffResult(output: string): ParsedDiffResult {
  const lines = output.split(/\r?\n/);
  const firstDiffLine = lines.findIndex(
    (line, index) => line.startsWith("--- ") && lines[index + 1]?.startsWith("+++ "),
  );
  if (firstDiffLine === -1) {
    return { summary: output.trim(), files: [] };
  }

  const summary = lines.slice(0, firstDiffLine).filter((line) => !line.startsWith("```")).join("\n").trim();
  const diffLines = lines.slice(firstDiffLine).filter((line) => !line.startsWith("```"));
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  let pendingOldPath: string | null = null;

  for (const line of diffLines) {
    if (line.startsWith("--- ")) {
      pendingOldPath = line.slice(4).trim();
      continue;
    }
    if (line.startsWith("+++ ")) {
      current = {
        oldPath: pendingOldPath ?? "",
        newPath: line.slice(4).trim(),
        lines: [],
      };
      files.push(current);
      pendingOldPath = null;
      continue;
    }
    if (!current) continue;
    if (line.startsWith("@@")) {
      current.lines.push({ kind: "hunk", text: line });
    } else if (line.startsWith("+")) {
      current.lines.push({ kind: "added", text: line.slice(1) });
    } else if (line.startsWith("-")) {
      current.lines.push({ kind: "removed", text: line.slice(1) });
    } else if (line.startsWith(" ")) {
      current.lines.push({ kind: "context", text: line.slice(1) });
    } else if (line.length > 0) {
      current.lines.push({ kind: "meta", text: line });
    }
  }

  return { summary, files };
}

function classForLine(kind: DiffLineKind): string {
  switch (kind) {
    case "added":
      return "bg-green-100 text-green-800 dark:bg-green-950/30 dark:text-green-200";
    case "removed":
      return "bg-red-100 text-red-800 dark:bg-red-950/30 dark:text-red-200";
    case "hunk":
      return "bg-blue-100 text-blue-800 dark:bg-blue-950/30 dark:text-blue-300";
    case "meta":
      return "text-gray-500";
    case "context":
      return "text-gray-300";
  }
}

function prefixForLine(kind: DiffLineKind): string {
  switch (kind) {
    case "added":
      return "+";
    case "removed":
      return "-";
    case "hunk":
      return "@";
    case "meta":
      return "";
    case "context":
      return "";
  }
}
