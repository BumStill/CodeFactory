// SPDX-License-Identifier: Apache-2.0
// @vitest-environment node

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

function source(path: string): string {
  return readFileSync(resolve(process.cwd(), path), "utf8");
}

function relativeLuminance([red, green, blue]: number[]): number {
  const [r, g, b] = [red, green, blue].map((channel) => {
    const value = channel / 255;
    return value <= 0.03928
      ? value / 12.92
      : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(foreground: number[], background: number[]): number {
  const foregroundLuminance = relativeLuminance(foreground);
  const backgroundLuminance = relativeLuminance(background);
  return (
    (Math.max(foregroundLuminance, backgroundLuminance) + 0.05) /
    (Math.min(foregroundLuminance, backgroundLuminance) + 0.05)
  );
}

function themeToken(css: string, theme: "light" | "dark", token: string): number[] {
  const themeBlock = css.match(
    new RegExp(`\\[data-theme="${theme}"\\]\\s*\\{([\\s\\S]*?)\\n\\}`),
  )?.[1];
  const value = themeBlock?.match(
    new RegExp(`--${token}:\\s*(\\d+)\\s+(\\d+)\\s+(\\d+);`),
  );
  if (!value) throw new Error(`Missing ${theme} theme token --${token}`);
  return value.slice(1).map(Number);
}

describe("agent workbench visual contract", () => {
  it("defines reusable semantic status tokens for both themes", () => {
    const css = source("src/styles/globals.css");
    for (const token of [
      "--status-progress",
      "--status-success",
      "--status-warning",
      "--status-danger",
      "--status-info",
    ]) {
      expect(css.match(new RegExp(token, "g"))?.length).toBeGreaterThanOrEqual(2);
    }
  });

  it("does not use 9px or 10px for the real session and task work surfaces", () => {
    for (const file of [
      "src/components/SessionSidebar.tsx",
      "src/components/DraftScopeBar.tsx",
      "src/components/MessageList.tsx",
      "src/components/WelcomeScreen.tsx",
      "src/components/GitChangesPanel.tsx",
      "src/components/CheckpointsPanel.tsx",
      "src/components/ToolCallCard.tsx",
      "src/components/GitStatusBar.tsx",
      "src/components/WelcomeUsageCard.tsx",
      "src/components/GitHistoryPanel.tsx",
      "src/components/RemoteGitPanel.tsx",
      "src/components/UpdaterBanner.tsx",
      "src/components/UpdateStatusPill.tsx",
      "src/pages/Workspace/WorkspacePage.tsx",
    ]) {
      expect(source(file), file).not.toMatch(/text-\[(?:9|10)px\]/);
    }
  });

  it("uses semantic status tokens for core MessageList states", () => {
    expect(source("src/components/MessageList.tsx")).not.toMatch(
      /(?:bg|border|text)-(?:amber|sky|green)-\d/,
    );
  });

  it("uses semantic status tokens instead of fixed chromatic shades in workbench status surfaces", () => {
    for (const file of [
      "src/components/ToolCallCard.tsx",
      "src/components/GitChangesPanel.tsx",
      "src/components/GitHistoryPanel.tsx",
      "src/components/GitStatusBar.tsx",
      "src/components/CheckpointsPanel.tsx",
      "src/components/RemoteGitPanel.tsx",
      "src/components/WelcomeUsageCard.tsx",
    ]) {
      expect(source(file), file).not.toMatch(
        /(?:bg|border|text)-(?:red|amber|yellow|green|emerald|blue|sky|cyan|orange|rose|lime|teal|indigo|violet|purple|pink|fuchsia)-\d/,
      );
    }
  });

  it("reserves outcome tokens for outcomes instead of tool and diff categories", () => {
    const toolCard = source("src/components/ToolCallCard.tsx");
    const toolStyles = toolCard.match(
      /function styleForTool[\s\S]*?\n}\n\nfunction toolLabel/,
    )?.[0] ?? "";
    expect(toolStyles).not.toMatch(
      /text-status-(?:progress|success|warning|danger)/,
    );

    const gitChanges = source("src/components/GitChangesPanel.tsx");
    const fileRowAndBadge = gitChanges.match(/function FileRow[\s\S]*$/)?.[0] ?? "";
    expect(fileRowAndBadge).not.toMatch(/text-status-(?:success|warning|danger)/);
  });

  it("treats open remote work as active and reserves success for merged outcomes", () => {
    const remote = source("src/components/RemoteGitPanel.tsx");
    expect(remote).not.toMatch(
      /issue\.state === "open"[\s\S]{0,160}?status-success/,
    );
    expect(remote).not.toContain('case "open": return "text-status-success"');
    expect(remote).toContain('case "merged": return "text-status-success"');
  });

  it("keeps checkpoint recovery available without a hover-only reveal", () => {
    expect(source("src/components/CheckpointsPanel.tsx")).not.toMatch(
      /opacity-0|group-hover:opacity-100|focus:opacity-100/,
    );
  });

  it("does not use decorative gray-700 for workbench text or controls", () => {
    for (const file of [
      "src/components/SessionSidebar.tsx",
      "src/components/WorkspaceDeliveryStatus.tsx",
      "src/components/GitChangesPanel.tsx",
      "src/components/CheckpointsPanel.tsx",
      "src/components/GitHistoryPanel.tsx",
      "src/components/RemoteGitPanel.tsx",
      "src/pages/Workspace/WorkspacePage.tsx",
    ]) {
      expect(source(file), file).not.toMatch(/text-gray-700/);
    }
  });

  it("disables decorative workbench animation when reduced motion is requested", () => {
    for (const file of [
      "src/components/ToolCallCard.tsx",
      "src/components/GitStatusBar.tsx",
      "src/components/WelcomeUsageCard.tsx",
      "src/components/GitHistoryPanel.tsx",
      "src/components/RemoteGitPanel.tsx",
      "src/components/UpdaterBanner.tsx",
      "src/components/UpdateStatusPill.tsx",
    ]) {
      const contents = source(file);
      for (const match of contents.matchAll(/\banimate-(?!none\b)[\w-]+/g)) {
        const offset = match.index ?? 0;
        const lineStart = contents.lastIndexOf("\n", offset) + 1;
        const nextLineBreak = contents.indexOf("\n", offset);
        const lineEnd = nextLineBreak === -1 ? contents.length : nextLineBreak;
        expect(contents.slice(lineStart, lineEnd), `${file}: ${match[0]}`).toContain(
          "motion-reduce:animate-none",
        );
      }
    }
  });

  it("does not describe through_release as live in product copy", () => {
    for (const file of [
      "src/pages/Settings/SettingsPage.tsx",
      "src/components/OnboardingWizard.tsx",
    ]) {
      expect(source(file), file).not.toMatch(/发布上线/);
    }
  });

  it("keeps semantic status text at WCAG AA contrast on its soft surface in both themes", () => {
    const css = source("src/styles/globals.css");
    for (const theme of ["light", "dark"] as const) {
      for (const status of [
        "progress",
        "success",
        "warning",
        "danger",
        "info",
      ]) {
        expect(
          contrast(
            themeToken(css, theme, `status-${status}`),
            themeToken(css, theme, `status-${status}-soft`),
          ),
          `${theme} ${status}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("keeps dark-theme muted 11–13px text at WCAG AA contrast on every work surface", () => {
    const css = source("src/styles/globals.css");
    for (const token of ["gray-500", "gray-600"]) {
      for (const surface of [
        "surface-0",
        "surface-1",
        "surface-2",
        "surface-3",
        "surface-4",
      ]) {
        expect(
          contrast(
            themeToken(css, "dark", token),
            themeToken(css, "dark", surface),
          ),
          `${token} on ${surface}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("keeps light-theme muted 11–13px text at WCAG AA contrast on every work surface", () => {
    const css = source("src/styles/globals.css");
    for (const token of ["gray-500", "gray-600"]) {
      for (const surface of [
        "surface-0",
        "surface-1",
        "surface-2",
        "surface-3",
        "surface-4",
      ]) {
        expect(
          contrast(
            themeToken(css, "light", token),
            themeToken(css, "light", surface),
          ),
          `${token} on ${surface}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});
