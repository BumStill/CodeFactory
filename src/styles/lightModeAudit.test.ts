// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node
//
// Light-mode contrast guard.
//
// Scans all .tsx components for known "dark-mode-only" text colors used
// without a `dark:` prefix. These break light mode (e.g. text-amber-200
// on a near-white surface gives ~1.3:1 contrast — invisible).
//
// Rule: any color shade in {100, 200, 300} from {amber, emerald, cyan,
// red, blue} used as `text-…` must be paired with a darker base color
// when no theme-prefix qualifier is present.
//
// This test was added after v0.5.1, where the chat surface looked broken
// in light mode because 9+ components used unconditional light text shades.

import { describe, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

// vitest runs from the project root, so `src/` resolves correctly.
const SRC = resolve(process.cwd(), "src");

// File-level allowlist: badges where the background is a SOLID dark color
// providing its own contrast for the light text (works in both themes).
// Note these and keep them out of the violation report.
// Self-contained badges: text sits on a fully-opaque colored surface that
// itself provides the contrast. These work in both themes by virtue of the
// solid colored chip being visible against any page background.
const ALLOWED_PATTERNS: { file: string; substring: string }[] = [
  { file: "EvidenceViewer.tsx",   substring: 'passed: "bg-green-800 text-green-200"' },
  { file: "EvidenceViewer.tsx",   substring: 'failed: "bg-red-800 text-red-200"' },
  { file: "EvidenceViewer.tsx",   substring: 'partial: "bg-yellow-800 text-yellow-200"' },
  { file: "RemoteGitPanel.tsx",   substring: 'bg-green-900 text-green-200' },
  { file: "RemoteGitPanel.tsx",   substring: 'bg-blue-900 text-blue-200' },
  { file: "SettingsPage.tsx",     substring: 'bg-orange-900 text-orange-200' },
  { file: "SpecsPage.tsx",        substring: 'review: "bg-yellow-800 text-yellow-200"' },
  { file: "SpecsPage.tsx",        substring: 'approved: "bg-green-800 text-green-200"' },
  { file: "SpecsPage.tsx",        substring: 'implementing: "bg-blue-800 text-blue-200"' },
  { file: "SpecsPage.tsx",        substring: 'bg-green-800 hover:bg-green-700 text-green-100' },
  { file: "SpecsPage.tsx",        substring: 'bg-blue-800 hover:bg-blue-700 text-blue-100' },
];

const FORBIDDEN_PATTERN = /text-(amber|emerald|cyan|red|blue|green|orange|yellow|pink|rose|indigo|violet|fuchsia|teal|sky|lime)-([12]00|300)\b/g;
const PREFIX_OK = /(dark:|hover:|focus:|group-hover:|peer-)/;

function* walk(dir: string): Generator<string> {
  for (const entry of readdirSync(dir)) {
    if (entry === "node_modules" || entry === "dist" || entry.startsWith(".")) continue;
    const full = join(dir, entry);
    const stat = statSync(full);
    if (stat.isDirectory()) {
      yield* walk(full);
    } else if (entry.endsWith(".tsx") && !entry.includes(".test.")) {
      yield full;
    }
  }
}

describe("light-mode contrast audit", () => {

  it("forbids unconditional dark-mode-only text shades in .tsx files", () => {
    const violations: { file: string; line: number; class: string; text: string }[] = [];

    for (const file of walk(SRC)) {
      const lines = readFileSync(file, "utf8").split("\n");
      lines.forEach((line: string, idx: number) => {
        const matches = [...line.matchAll(FORBIDDEN_PATTERN)];
        if (matches.length === 0) return;

        // Skip if this line is in the allowlist
        const rel = file.replace(SRC + "/", "");
        const isAllowed = ALLOWED_PATTERNS.some(
          (a) => rel.endsWith(a.file) && line.includes(a.substring),
        );
        if (isAllowed) return;

        for (const m of matches) {
          // Look back ~8 chars before the match to see if a `dark:` or
          // `hover:` prefix qualifier directly precedes it.
          const start = Math.max(0, m.index! - 8);
          const before = line.slice(start, m.index!);
          if (PREFIX_OK.test(before)) continue;

          violations.push({
            file: rel,
            line: idx + 1,
            class: m[0],
            text: line.trim(),
          });
        }
      });
    }

    if (violations.length > 0) {
      const summary = violations
        .map((v) => `  ${v.file}:${v.line}  ${v.class}\n    ${v.text}`)
        .join("\n");
      throw new Error(
        `Found ${violations.length} unconditional dark-mode-only text colors. ` +
          `Pair with a darker base (e.g. text-amber-700 dark:text-amber-200) ` +
          `or add to ALLOWED_PATTERNS if the background is self-contained:\n${summary}`,
      );
    }
  });

});
