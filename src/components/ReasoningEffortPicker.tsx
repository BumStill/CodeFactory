// SPDX-License-Identifier: Apache-2.0
//
// Compact "reasoning effort" control for reasoning-capable models. Surfaces
// ONLY when the active endpoint is the ChatGPT/Codex subscription — the only
// path that honours `reasoning.effort` — in line with the state-driven layout
// principle (show a control where it's relevant, hide it otherwise). Sets the
// ACTIVE SESSION's per-session override (the global default lives in Settings).
import { useSettingsStore } from "../stores/settings";
import { useChatStore } from "../stores/chat";
import type { Settings, ReasoningEffort } from "../lib/tauri";

const LEGACY_EFFORTS: ReasoningEffort[] = ["minimal", "low", "medium", "high", "xhigh"];
const LABELS: Record<ReasoningEffort, string> = {
  minimal: "最简",
  low: "低",
  medium: "中",
  high: "高",
  xhigh: "超高",
  max: "最大",
  ultra: "极致",
};

/** Whether the reasoning control is relevant for the current settings — only
 *  the ChatGPT/Codex endpoint honours reasoning.effort. Exported for testing. */
export function reasoningPickerVisible(settings: Settings | null): boolean {
  // Defensive: settings (or endpoints) may be partial while loading.
  const ep = settings?.endpoints?.[settings.default_endpoint];
  return ep?.api_style === "chatgpt";
}

export function reasoningEffortsForModel(
  settings: Settings | null,
  modelId: string,
): ReasoningEffort[] {
  const endpoint = settings?.endpoints?.[settings.default_endpoint];
  const model = endpoint?.custom_models?.find((candidate) => candidate.id === modelId);
  return model?.supported_reasoning_efforts?.length
    ? model.supported_reasoning_efforts
    : LEGACY_EFFORTS;
}

function effectiveEffort(
  requested: ReasoningEffort,
  supported: ReasoningEffort[],
  fallback?: ReasoningEffort,
): ReasoningEffort {
  if (supported.includes(requested)) return requested;
  if (fallback && supported.includes(fallback)) return fallback;
  if (supported.includes("medium")) return "medium";
  return supported[0] ?? requested;
}

export function ReasoningEffortPicker() {
  const settings = useSettingsStore((s) => s.settings);
  const activeSession = useChatStore((s) => s.activeSession);
  const setEffort = useChatStore((s) => s.updateActiveSessionReasoningEffort);
  if (!settings || !reasoningPickerVisible(settings) || !activeSession) return null;
  // Per-session override; falls back to the global default for display.
  const globalDefault: ReasoningEffort = settings.reasoning_effort ?? "medium";
  const requested: ReasoningEffort =
    (activeSession.reasoning_effort as ReasoningEffort | null | undefined) ?? globalDefault;
  const endpoint = settings.endpoints[settings.default_endpoint];
  const model = endpoint?.custom_models?.find((candidate) => candidate.id === activeSession.model_id);
  const efforts = reasoningEffortsForModel(settings, activeSession.model_id);
  const effort = effectiveEffort(requested, efforts, model?.default_reasoning_effort);
  return (
    <select
      value={effort}
      onChange={(e) => void setEffort(e.target.value as ReasoningEffort)}
      title="思考强度 (reasoning effort) — 仅作用于当前会话，立即对后续请求生效"
      className="rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300 transition-colors hover:bg-surface-3"
    >
      {efforts.map((v) => (
        <option key={v} value={v}>
          思考·{LABELS[v]}
        </option>
      ))}
    </select>
  );
}
