// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { setTheme as setNativeAppTheme } from "@tauri-apps/api/app";
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

let _mediaQuery: MediaQueryList | null = null;
let _mediaListener: (() => void) | null = null;
let _themeApplyVersion = 0;

function resolveTheme(theme: Theme): "dark" | "light" {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }
  return theme;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function toNativeTheme(theme: Theme): "dark" | "light" | null {
  return theme === "system" ? null : theme;
}

async function syncNativeTheme(theme: Theme): Promise<void> {
  if (!isTauriRuntime()) return;

  try {
    await setNativeAppTheme(toNativeTheme(theme));
  } catch (error) {
    console.warn("Failed to sync native app theme", error);
  }
}

export function applyTheme(settings: Settings) {
  const html = document.documentElement;
  const applyVersion = ++_themeApplyVersion;

  // ── Remove previous media listener if any ───────────────────────────────
  if (_mediaQuery && _mediaListener) {
    _mediaQuery.removeEventListener("change", _mediaListener);
    _mediaQuery = null;
    _mediaListener = null;
  }

  // ── Apply theme ─────────────────────────────────────────────────────────
  const applyResolved = () => {
    html.setAttribute("data-theme", resolveTheme(settings.theme));
  };

  applyResolved();
  void syncNativeTheme(settings.theme).then(() => {
    if (settings.theme === "system" && applyVersion === _themeApplyVersion) {
      applyResolved();
    }
  });

  if (settings.theme === "system") {
    _mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    _mediaListener = applyResolved;
    _mediaQuery.addEventListener("change", _mediaListener);
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
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
  settings: null,

  load: async () => {
    const s = await invoke<Settings>("get_settings");
    // Fill in defaults for older configs that predate these fields.
    // `onboarded` left as-is — missing/false triggers the first-run overlay,
    // which is the correct behaviour for both fresh installs and old
    // upgrade-from-pre-onboarding-feature users (they get one trip
    // through the wizard, then the flag persists).
    const merged: Settings = {
      ...s,
      theme: s.theme ?? "dark",
      font_family: s.font_family ?? "inter",
      font_size: s.font_size ?? 14,
      remote_postmortem_enabled: s.remote_postmortem_enabled ?? false,
      default_model_policy: s.default_model_policy ?? "prefer",
    };
    applyTheme(merged);
    set({ settings: merged });
  },

  save: async (s) => {
    const authoritative = await invoke<Settings>("save_settings", { newSettings: s });
    applyTheme(authoritative);
    set({ settings: authoritative });
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
}));
