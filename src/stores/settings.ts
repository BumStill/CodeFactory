// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { invoke } from "../lib/tauri";
import type { Settings, Theme } from "../lib/tauri";

// ── Font family map ──────────────────────────────────────────────────────────

export const FONT_FAMILIES: Record<string, string> = {
  inter:          "Inter, system-ui, sans-serif",
  system:         "system-ui, -apple-system, sans-serif",
  "jetbrains-mono": "'JetBrains Mono', Consolas, Menlo, monospace",
};

export const FONT_FAMILY_LABELS: Record<string, string> = {
  inter:            "Inter",
  system:           "System UI",
  "jetbrains-mono": "JetBrains Mono",
};

export const FONT_SIZE_MIN = 12;
export const FONT_SIZE_MAX = 20;

// ── Theme application ────────────────────────────────────────────────────────

let _mediaListener: (() => void) | null = null;

function resolveTheme(theme: Theme): "dark" | "light" {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }
  return theme;
}

export function applyTheme(settings: Settings) {
  const html = document.documentElement;

  // ── Remove previous media listener if any ───────────────────────────────
  if (_mediaListener) {
    window
      .matchMedia("(prefers-color-scheme: dark)")
      .removeEventListener("change", _mediaListener);
    _mediaListener = null;
  }

  // ── Apply theme ─────────────────────────────────────────────────────────
  const applyResolved = () => {
    html.setAttribute("data-theme", resolveTheme(settings.theme));
  };

  applyResolved();

  if (settings.theme === "system") {
    _mediaListener = applyResolved;
    window
      .matchMedia("(prefers-color-scheme: dark)")
      .addEventListener("change", _mediaListener);
  }

  // ── Apply font ──────────────────────────────────────────────────────────
  const fontStack = FONT_FAMILIES[settings.font_family] ?? FONT_FAMILIES.inter;
  html.style.setProperty("--font-family", fontStack);
  html.style.setProperty("--font-size", `${settings.font_size}px`);
}

// ── Store ────────────────────────────────────────────────────────────────────

interface SettingsStore {
  settings: Settings | null;
  load: () => Promise<void>;
  save: (s: Settings) => Promise<void>;
  setTheme: (theme: Theme) => Promise<void>;
  setFontFamily: (family: string) => Promise<void>;
  setFontSize: (size: number) => Promise<void>;
  saveApiKey: (keyRef: string, value: string) => Promise<void>;
  getApiKey: (keyRef: string) => Promise<string | null>;
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  settings: null,

  load: async () => {
    const s = await invoke<Settings>("get_settings");
    // Fill in defaults for older configs that predate these fields
    const merged: Settings = {
      ...s,
      theme: s.theme ?? "dark",
      font_family: s.font_family ?? "inter",
      font_size: s.font_size ?? 14,
    };
    applyTheme(merged);
    set({ settings: merged });
  },

  save: async (s) => {
    await invoke("save_settings", { newSettings: s });
    applyTheme(s);
    set({ settings: s });
  },

  setTheme: async (theme) => {
    const { settings, save } = get();
    if (!settings) return;
    await save({ ...settings, theme });
  },

  setFontFamily: async (font_family) => {
    const { settings, save } = get();
    if (!settings) return;
    await save({ ...settings, font_family });
  },

  setFontSize: async (font_size) => {
    const { settings, save } = get();
    if (!settings) return;
    await save({ ...settings, font_size });
  },

  saveApiKey: async (keyRef, value) => {
    await invoke("save_api_key", { keyRef, value });
  },

  getApiKey: async (keyRef) => {
    return invoke<string | null>("get_api_key", { keyRef });
  },
}));
