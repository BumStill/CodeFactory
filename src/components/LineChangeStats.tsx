// SPDX-License-Identifier: Apache-2.0
//
// M36 — one place that renders "how many lines changed", so the tool row, the
// turn total and the real-browser acceptance page cannot drift apart.
// Green for additions and red for deletions follow the git/editor convention;
// both come from the theme tokens, so light and dark mode stay legible.

import { isEmptyLineChange } from "../lib/editLineStats";
import type { LineChangeStats as LineStats, ToolLineStats } from "../lib/editLineStats";

interface Props {
  stats: LineStats;
  className?: string;
}

/** `+X −Y`, or nothing at all when the call changed no lines. */
export function LineChangeStats({ stats, className }: Props) {
  if (isEmptyLineChange(stats)) return null;
  return (
    <span
      data-testid="line-change-stats"
      data-added={stats.added}
      data-removed={stats.removed}
      className={`shrink-0 whitespace-nowrap font-mono tabular-nums ${className ?? ""}`}
    >
      <span className="text-status-success">+{stats.added}</span>{" "}
      <span className="text-status-danger">−{stats.removed}</span>
    </span>
  );
}

/** The row badge for one edit/write call. */
export function ToolLineStatsBadge({ stats }: { stats: ToolLineStats }) {
  if (stats.kind === "edit") {
    return <LineChangeStats stats={stats} />;
  }
  if (stats.added === 0) return null;
  // No prior content in the arguments, so never claim deleted lines: a
  // created file is `+N`, an overwrite is just how many lines were written.
  return stats.newFile ? (
    <span
      data-testid="line-change-stats"
      data-added={stats.added}
      data-new-file="true"
      className="shrink-0 whitespace-nowrap font-mono tabular-nums text-status-success"
    >
      +{stats.added}
    </span>
  ) : (
    <span
      data-testid="written-line-count"
      className="shrink-0 whitespace-nowrap font-mono tabular-nums text-gray-500"
    >
      写入 {stats.added} 行
    </span>
  );
}
