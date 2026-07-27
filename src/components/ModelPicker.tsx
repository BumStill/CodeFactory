// SPDX-License-Identifier: Apache-2.0
import { useState, useRef, useEffect } from "react";
import { ChevronDown, Sparkles } from "lucide-react";
import { useChatStore } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import { invoke } from "../lib/tauri";

export function ModelPicker() {
  const {
    models,
    activeModel,
    activeSession,
    updateActiveSessionModel,
    updateActiveSessionModelConfig,
    loadModels,
    setModel,
  } = useChatStore();
  const { settings, load: reloadSettings, save: saveSettings } = useSettingsStore();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [loadingEndpoint, setLoadingEndpoint] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  // When the active endpoint changes:
  //   1. Reload the model list for that endpoint
  //   2. Pull the endpoint's remembered active_model and swap the chat
  //      store to it — so we don't carry over a vendor-prefixed id from a
  //      previous OpenRouter session into a direct-DeepSeek run, which
  //      was the root cause of the v0.3.5 400 reports.
  useEffect(() => {
    const ep = activeSession?.endpoint_id ?? settings?.default_endpoint ?? "openrouter";
    let cancelled = false;
    setLoadingEndpoint(ep);
    loadModels(ep).finally(() => {
      if (!cancelled) setLoadingEndpoint(null);
    });
    // An existing session owns its model independently. The endpoint's
    // remembered default is only a seed for a not-yet-materialized draft;
    // applying it here would make opening a session silently display and use
    // a different model from the one persisted on that session.
    if (!activeSession) {
      invoke<string>("get_endpoint_active_model", { endpointName: ep })
        .then((m) => {
          if (!cancelled && m && m !== activeModel) setModel(m);
        })
        .catch(() => { /* first run / endpoint without a saved model */ });
    }
    return () => {
      cancelled = true;
    };
  }, [activeSession?.endpoint_id, settings?.default_endpoint]);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, []);

  const displayed = activeModel.split("/").pop() ?? activeModel;
  const activeEndpoint =
    activeSession?.endpoint_id ?? settings?.default_endpoint ?? "openrouter";
  const activePolicy =
    activeSession?.model_policy ?? settings?.default_model_policy ?? "prefer";
  const endpointKeys = settings ? Object.keys(settings.endpoints).sort() : [];
  const modelListLoading = loadingEndpoint !== null;
  const filtered = models
    .filter(
      (m) =>
        !query ||
        m.id.toLowerCase().includes(query.toLowerCase()) ||
        m.name.toLowerCase().includes(query.toLowerCase())
    )
    .sort((a, b) => {
      // Custom models first, then alphabetical by id
      if (!!a.is_custom !== !!b.is_custom) return a.is_custom ? -1 : 1;
      return a.id.localeCompare(b.id);
    });

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1 rounded px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-surface-3 transition-colors"
        title={`${activeEndpoint} / ${activeModel} · ${activePolicy}`}
      >
        <span className="max-w-[190px] truncate">
          {activeEndpoint} / {displayed} · {
            activePolicy === "fixed" ? "固定" : activePolicy === "auto" ? "自动" : "首选"
          }
        </span>
        <ChevronDown size={12} />
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1 z-50 w-72 rounded-lg border border-border bg-surface-2 shadow-xl">
          <div className="space-y-2 p-2 border-b border-border">
            {activeSession && (
              <>
                <select
                  aria-label="模型策略"
                  value={activePolicy}
                  onChange={(event) => {
                    void updateActiveSessionModelConfig({
                      endpointId: activeEndpoint,
                      modelId: activeModel,
                      policy: event.target.value as "fixed" | "prefer" | "auto",
                    });
                  }}
                  className="w-full rounded bg-surface-3 px-2 py-1 text-xs text-gray-200 outline-none"
                >
                  <option value="fixed">固定 · 只使用当前模型</option>
                  <option value="prefer">首选 · 安全时允许兼容接管</option>
                  <option value="auto">自动 · 按能力与状态选择</option>
                </select>
                <p className="px-0.5 text-xs leading-5 text-gray-500">
                  会话策略更改只从下一轮开始生效；当前运行中的回合不会改路。
                </p>
              </>
            )}
            {endpointKeys.length > 1 && (
              <select
                aria-label="模型端点"
                value={activeEndpoint}
                onChange={async (e) => {
                  if (!settings) return;
                  const endpointName = e.target.value;
                  setLoadingEndpoint(endpointName);
                  setQuery("");
                  try {
                    const modelsLoading = loadModels(endpointName);
                    const model = await invoke<string>("get_endpoint_active_model", {
                      endpointName,
                    }).catch(() => "");
                    if (model) {
                      if (activeSession) {
                        await updateActiveSessionModelConfig({
                          endpointId: endpointName,
                          modelId: model,
                          policy: activePolicy,
                        });
                      } else {
                        await saveSettings({
                          ...settings,
                          default_endpoint: endpointName,
                          default_model: model,
                        });
                        setModel(model);
                      }
                    }
                    await modelsLoading;
                    await reloadSettings();
                  } finally {
                    setLoadingEndpoint(null);
                  }
                }}
                className="w-full bg-surface-3 rounded px-2 py-1 text-xs text-gray-200 outline-none"
              >
                {endpointKeys.map((key) => (
                  <option key={key} value={key}>{key}</option>
                ))}
              </select>
            )}
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              disabled={modelListLoading}
              placeholder="搜索模型…"
              className="w-full bg-surface-3 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none disabled:opacity-50"
            />
          </div>
          <ul className="max-h-64 overflow-y-auto py-1">
            {modelListLoading ? (
              <li className="px-3 py-2 text-xs text-gray-600">正在加载模型…</li>
            ) : filtered.slice(0, 50).map((m) => (
              <li key={m.id}>
                <button
                  className={`flex w-full items-center gap-1.5 px-3 py-1.5 text-left text-xs hover:bg-surface-3 transition-colors ${
                    m.id === activeModel ? "text-accent" : "text-gray-300"
                  }`}
                  onClick={async () => {
                    if (activeSession) {
                      await updateActiveSessionModelConfig({
                        endpointId: activeEndpoint,
                        modelId: m.id,
                        policy: activePolicy,
                      });
                    } else {
                      // Outside an existing session this remains the
                      // new-session default for the endpoint.
                      await invoke("set_endpoint_active_model", {
                        endpointName: activeEndpoint,
                        modelId: m.id,
                      }).catch(() => { /* best-effort */ });
                      await updateActiveSessionModel(m.id);
                      await reloadSettings();
                    }
                    setOpen(false);
                  }}
                  title={m.id}
                >
                  {m.is_custom && (
                    <Sparkles
                      size={10}
                      className="shrink-0 text-amber-400"
                    />
                  )}
                  <span className="truncate">{m.id}</span>
                </button>
              </li>
            ))}
            {!modelListLoading && filtered.length === 0 && (
              <li className="px-3 py-2 text-xs text-gray-600">未找到模型</li>
            )}
          </ul>
        </div>
      )}
    </div>
  );
}
