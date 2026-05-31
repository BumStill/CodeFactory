// SPDX-License-Identifier: Apache-2.0
//
// Compact "reasoning effort" control for reasoning-capable models. Surfaces
// ONLY when the active endpoint is the ChatGPT/Codex subscription — the only
// path that honours `reasoning.effort` — in line with the state-driven layout
// principle (show a control where it's relevant, hide it otherwise). Edits the
// persisted global default in Settings.
import { useSettingsStore } from "../stores/settings";
import type { Settings, ReasoningEffort } from "../lib/tauri";

const EFFORTS: ReasoningEffort[] = ["minimal", "low", "medium", "high"];
const LABELS: Record<ReasoningEffort, string> = {
  minimal: "最简",
  low: "低",
  medium: "中",
  high: "高",
};

/** Whether the reasoning control is relevant for the current settings — only
 *  the ChatGPT/Codex endpoint honours reasoning.effort. Exported for testing. */
export function reasoningPickerVisible(settings: Settings | null): boolean {
  if (!settings) return false;
  const ep = settings.endpoints[settings.default_endpoint];
  return ep?.api_style === "chatgpt";
}

export function ReasoningEffortPicker() {
  const settings = useSettingsStore((s) => s.settings);
  const save = useSettingsStore((s) => s.save);
  if (!settings || !reasoningPickerVisible(settings)) return null;
  const effort: ReasoningEffort = settings.reasoning_effort ?? "medium";
  return (
    <select
      value={effort}
      onChange={(e) =>
        void save({ ...settings, reasoning_effort: e.target.value as ReasoningEffort })
      }
      title="思考强度 (reasoning effort) — 默认值，立即对后续请求生效"
      className="rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300 transition-colors hover:bg-surface-3"
    >
      {EFFORTS.map((v) => (
        <option key={v} value={v}>
          思考·{LABELS[v]}
        </option>
      ))}
    </select>
  );
}
