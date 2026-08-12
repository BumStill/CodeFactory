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
// DeepSeek's chat/completions API accepts exactly these three levels
// (medium/xhigh are compatibility-mapped server-side).
const DEEPSEEK_EFFORTS: ReasoningEffort[] = ["low", "high", "max"];
const LABELS: Record<ReasoningEffort, string> = {
  minimal: "最简",
  low: "低",
  medium: "中",
  high: "高",
  xhigh: "超高",
  max: "最大",
  ultra: "极致",
};

function isDeepSeekEndpoint(settings: Settings | null, endpointId?: string): boolean {
  const ep = settings?.endpoints?.[endpointId ?? settings.default_endpoint];
  if (!ep) return false;
  const base = (ep.base_url ?? "").toLowerCase();
  const models = ep.custom_models ?? [];
  return (
    base.includes("deepseek.com") ||
    models.some((m) => m.id.toLowerCase().startsWith("deepseek"))
  );
}

/** Whether the reasoning control is relevant for the current settings — the
 *  ChatGPT/Codex endpoint honours reasoning.effort, and DeepSeek models
 *  (deepseek.com direct or deepseek/… via OpenRouter) accept reasoning_effort
 *  low|high|max. Exported for testing. */
export function reasoningPickerVisible(settings: Settings | null, endpointId?: string): boolean {
  // Defensive: settings (or endpoints) may be partial while loading.
  const ep = settings?.endpoints?.[endpointId ?? settings.default_endpoint];
  return ep?.api_style === "chatgpt" || isDeepSeekEndpoint(settings, endpointId);
}

export function reasoningEffortsForModel(
  settings: Settings | null,
  modelId: string,
  endpointId?: string,
): ReasoningEffort[] {
  const endpoint = settings?.endpoints?.[endpointId ?? settings.default_endpoint];
  const model = endpoint?.custom_models?.find((candidate) => candidate.id === modelId);
  if (model?.supported_reasoning_efforts?.length) {
    return model.supported_reasoning_efforts;
  }
  if (modelId.toLowerCase().startsWith("deepseek")) return DEEPSEEK_EFFORTS;
  return LEGACY_EFFORTS;
}

function effectiveEffort(
  requested: ReasoningEffort,
  supported: ReasoningEffort[],
  fallback?: ReasoningEffort,
): ReasoningEffort {
  // v1.46.0 could persist the catalog-only `ultra` label even though the
  // ChatGPT Responses transport accepts `max` as its highest request value.
  if (requested === "ultra" && supported.includes("max")) return "max";
  if (supported.includes(requested)) return requested;
  if (fallback && supported.includes(fallback)) return fallback;
  if (supported.includes("medium")) return "medium";
  return supported[0] ?? requested;
}

export function ReasoningEffortPicker({ embedded = false }: { embedded?: boolean } = {}) {
  const settings = useSettingsStore((s) => s.settings);
  const activeSession = useChatStore((s) => s.activeSession);
  const setEffort = useChatStore((s) => s.updateActiveSessionReasoningEffort);
  if (!settings || !activeSession) return null;
  // Legacy rows may not yet own an endpoint. Prefer the persisted session
  // endpoint whenever present; only those unresolved rows inherit the default.
  const endpointId = activeSession.endpoint_id ?? settings.default_endpoint;
  if (!reasoningPickerVisible(settings, endpointId)) return null;
  // Per-session override; falls back to the global default for display.
  const globalDefault: ReasoningEffort = settings.reasoning_effort ?? "medium";
  const requested: ReasoningEffort =
    (activeSession.reasoning_effort as ReasoningEffort | null | undefined) ?? globalDefault;
  const endpoint = settings.endpoints[endpointId];
  const model = endpoint?.custom_models?.find((candidate) => candidate.id === activeSession.model_id);
  const efforts = reasoningEffortsForModel(settings, activeSession.model_id, endpointId);
  const effort = effectiveEffort(requested, efforts, model?.default_reasoning_effort);
  const select = (
    <select
      aria-label="下一回合思考强度"
      value={effort}
      onChange={(e) => void setEffort(e.target.value as ReasoningEffort)}
      title="思考强度 (reasoning effort) — 仅作用于当前会话，从下一回合开始生效"
      className={embedded
        ? "min-h-[44px] w-full rounded border border-control-border bg-surface-3 px-2 py-1 text-xs text-gray-200 outline-none focus:border-accent focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
        : "min-h-[44px] rounded-lg border border-control-border bg-surface-2 px-2 py-1 text-xs text-gray-300 transition-colors hover:bg-surface-3 focus:outline-none focus:border-accent focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"}
    >
      {efforts.map((v) => (
        <option key={v} value={v}>
          思考·{LABELS[v]}
        </option>
      ))}
    </select>
  );
  if (!embedded) return select;
  return (
    <div className="space-y-1">
      <p className="px-0.5 text-[11px] font-medium text-gray-500">思考强度</p>
      {select}
    </div>
  );
}
