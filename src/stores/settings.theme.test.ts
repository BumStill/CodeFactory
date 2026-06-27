// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Settings, Theme } from "../lib/tauri";

const { setNativeThemeMock } = vi.hoisted(() => ({
  setNativeThemeMock: vi.fn(() => Promise.resolve()),
}));

vi.mock("@tauri-apps/api/app", () => ({
  setTheme: setNativeThemeMock,
}));

import { applyTheme } from "./settings";

function settings(theme: Theme): Settings {
  return {
    endpoints: {},
    default_endpoint: "openrouter",
    default_model: "openai/gpt-4o",
    permissions: { allow: [], ask: [], deny: [], full_access: false },
    shell: { shell: "zsh" },
    auto_create_pr: false,
    theme,
    font_family: "inter",
    font_size: 14,
    onboarded: true,
  };
}

function installMatchMedia(initialMatches: boolean) {
  let matches = initialMatches;
  const listeners = new Set<() => void>();
  const mediaQuery = {
    get matches() {
      return matches;
    },
    media: "(prefers-color-scheme: dark)",
    addEventListener: vi.fn((_event: string, listener: () => void) => {
      listeners.add(listener);
    }),
    removeEventListener: vi.fn((_event: string, listener: () => void) => {
      listeners.delete(listener);
    }),
  } as unknown as MediaQueryList;

  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: vi.fn(() => mediaQuery),
  });

  return {
    mediaQuery,
    setMatches(next: boolean) {
      matches = next;
      listeners.forEach((listener) => listener());
    },
  };
}

function markTauriRuntime() {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: { metadata: { currentWindow: { label: "main" } } },
  });
}

afterEach(() => {
  document.documentElement.removeAttribute("data-theme");
  document.documentElement.removeAttribute("style");
  delete (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  setNativeThemeMock.mockClear();
});

describe("applyTheme", () => {
  it("resolves system theme from the OS media query and follows changes", () => {
    const media = installMatchMedia(false);

    applyTheme(settings("system"));

    expect(document.documentElement.getAttribute("data-theme")).toBe("light");

    media.setMatches(true);

    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("removes the system listener when switching to an explicit theme", () => {
    const media = installMatchMedia(false);

    applyTheme(settings("system"));
    applyTheme(settings("light"));
    media.setMatches(true);

    expect(media.mediaQuery.removeEventListener).toHaveBeenCalled();
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
  });

  it("sets native app theme to auto for system and explicit values for overrides", async () => {
    installMatchMedia(false);
    markTauriRuntime();

    applyTheme(settings("dark"));
    applyTheme(settings("light"));
    applyTheme(settings("system"));
    await vi.waitFor(() => {
      expect(setNativeThemeMock).toHaveBeenCalledWith("dark");
      expect(setNativeThemeMock).toHaveBeenCalledWith("light");
      expect(setNativeThemeMock).toHaveBeenCalledWith(null);
    });
  });

  it("does not call the native app API outside Tauri", () => {
    installMatchMedia(false);

    applyTheme(settings("system"));

    expect(setNativeThemeMock).not.toHaveBeenCalled();
  });
});
