// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";
import { setTheme as setNativeAppTheme } from "@tauri-apps/api/app";
import { invoke } from "../lib/tauri";
import type { Settings, Theme } from "../lib/tauri";

// ── Font family map ──────────────────────────────────────────────────────────

/**
 * UI typefaces.
 *
 * The CJK families are named explicitly rather than left to `system-ui`. This
 * is a `lang="zh-CN"` product, and letting the platform pick meant Chinese
 * rendered in whatever each OS happened to fall back to.
 *
 * "Inter Variable" is the family fontsource registers; it ships with the app
 * (see main.tsx), so this option is no longer a wish.
 */
export const FONT_FAMILIES: Record<string, string> = {
  inter:  "'Inter Variable', -apple-system, 'PingFang SC', 'Microsoft YaHei UI', system-ui, sans-serif",
  system: "-apple-system, system-ui, 'PingFang SC', 'Microsoft YaHei UI', sans-serif",
};

export const FONT_FAMILY_LABELS: Record<string, string> = {
  inter:  "Inter",
  system: "系统默认",
};

/**
 * Monospace typefaces — code, paths, terminal output.
 *
 * Separate from the UI font on purpose. JetBrains Mono used to be offered as
 * an *interface* font, and it has no CJK glyphs: choosing it turned the app
 * into monospace Latin mixed with proportional Chinese.
 */
export const MONO_FONT_FAMILIES: Record<string, string> = {
  "jetbrains-mono": "'JetBrains Mono Variable', ui-monospace, 'SF Mono', Consolas, Menlo, monospace",
  system:           "ui-monospace, 'SF Mono', Consolas, Menlo, monospace",
};

export const MONO_FONT_FAMILY_LABELS: Record<string, string> = {
  "jetbrains-mono": "JetBrains Mono",
  system:           "系统等宽",
};

export const FONT_SIZE_MIN = 12;
export const FONT_SIZE_MAX = 20;
/**
 * The body size the typography scale is authored against.
 *
 * `--font-scale` is `font_size / FONT_SIZE_BASE`, so the default setting is
 * exactly 1 and every token renders at its authored px value.
 */
export const FONT_SIZE_BASE = 14;

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
  html.style.setProperty("--font-family", FONT_FAMILIES[settings.font_family] ?? FONT_FAMILIES.inter);
  html.style.setProperty(
    "--font-family-mono",
    MONO_FONT_FAMILIES[settings.mono_font_family] ?? MONO_FONT_FAMILIES["jetbrains-mono"],
  );
  // Text size drives a scale factor, never the rem baseline. Writing the user's
  // size onto `html { font-size }` used to resize spacing, radii and icon boxes
  // along with the text, because Tailwind expresses all of them in rem — while
  // hard-coded px sizes stayed put. Only the font-size tokens read this.
  html.style.setProperty("--font-scale", String(settings.font_size / FONT_SIZE_BASE));
}

// ── Store ────────────────────────────────────────────────────────────────────

interface SettingsStore {
  settings: Settings | null;
  load: () => Promise<void>;
  save: (s: Settings) => Promise<void>;
  setTheme: (theme: Theme) => Promise<void>;
  setFontFamily: (family: string) => Promise<void>;
  setMonoFontFamily: (family: string) => Promise<void>;
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
      // "jetbrains-mono" used to be a valid UI font. Anyone carrying that
      // setting forward gets the intent honoured on the axis where it makes
      // sense — monospace for code — instead of an unresolvable UI stack.
      font_family: s.font_family === "jetbrains-mono" ? "inter" : s.font_family ?? "inter",
      mono_font_family: s.mono_font_family ?? "jetbrains-mono",
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

  setMonoFontFamily: async (mono_font_family) => {
    const { settings, save } = get();
    if (!settings) return;
    await save({ ...settings, mono_font_family });
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
