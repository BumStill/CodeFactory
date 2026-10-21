// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronDown, Sparkles } from "lucide-react";
import { useChatStore } from "../stores/chat";
import { useSettingsStore } from "../stores/settings";
import { refreshChatGptCatalogIfStale } from "../stores/chatgptCatalog";
import { evaluateWindow, modelWindowKey } from "../lib/usageWindows";
import { invoke } from "../lib/tauri";
import { ReleaseUrgencyPicker } from "./ReleaseUrgencyPicker";
import { ReasoningEffortPicker } from "./ReasoningEffortPicker";
import { UsageWindowDialog } from "./UsageWindowDialog";

interface ModelPickerProps {
  /** Render the menu in document.body so a draft composer cannot clip it. */
  portal?: boolean;
  /** Give the draft-context control a clearer visual affordance. */
  prominent?: boolean;
}

export function ModelPicker({ portal = false, prominent = false }: ModelPickerProps) {
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
  // CF-WINDOW-R6（M86 返修）：不在时段内的模型不再用 window.alert/confirm 提示，
  // 而是走应用内对话框（原生弹窗在 Tauri WebView 里不保证弹得出来）。
  const [windowPrompt, setWindowPrompt] = useState<{ endpoint: string; model: string; nextTransitionMs: number | null } | null>(null);
  // 本会话已持久化的时段豁免（重启后仍在），用来提供撤销入口。
  const [sessionOverrides, setSessionOverrides] = useState<string[]>([]);
  const ref = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = `model-picker-menu-${useId().replace(/:/g, "")}`;
  const [portalPosition, setPortalPosition] = useState({ left: 8, top: 8, maxHeight: 360 });

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
      const target = e.target as Node;
      if (ref.current?.contains(target) || menuRef.current?.contains(target)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, []);

  const closeAndRestoreFocus = useCallback(() => {
    setOpen(false);
    triggerRef.current?.focus();
  }, []);

  // Opening the menu is the one moment the list has to be right, so it is
  // also the moment to ask the server. The catalog was otherwise fetched only
  // at startup and on the Settings page: an app left running never learned
  // about a newly published model, and re-picking the endpoint only re-read
  // what startup had already written. The call is TTL-guarded, single-flight,
  // and returns immediately when signed out.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    // `activeEndpoint` is declared below this effect, so resolve the same
    // expression here rather than reading it from the temporal dead zone.
    const endpoint = activeSession?.endpoint_id ?? settings?.default_endpoint ?? "openrouter";
    void refreshChatGptCatalogIfStale().then(() => {
      if (!cancelled) void loadModels(endpoint);
    });
    return () => {
      cancelled = true;
    };
  }, [open, activeSession?.endpoint_id, settings?.default_endpoint, loadModels]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeAndRestoreFocus();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [closeAndRestoreFocus, open]);

  const updatePortalPosition = useCallback(() => {
    if (!portal || typeof window === "undefined") return;
    const triggerRect = ref.current?.getBoundingClientRect();
    if (!triggerRect) return;
    const composerRect = ref.current
      ?.closest<HTMLElement>('[data-testid="message-input-control-row"]')
      ?.getBoundingClientRect();
    const upperAnchor = composerRect?.top ?? triggerRect.top;
    const viewportPadding = 8;
    const gap = 4;
    const menuWidth = Math.min(288, Math.max(0, window.innerWidth - viewportPadding * 2));
    const availableAbove = Math.max(0, upperAnchor - gap - viewportPadding);
    const availableBelow = Math.max(
      0,
      window.innerHeight - triggerRect.bottom - gap - viewportPadding,
    );
    const measuredHeight = menuRef.current?.getBoundingClientRect().height || 360;
    const opensBelow = availableBelow >= measuredHeight || availableBelow > availableAbove;
    const availableHeight = opensBelow ? availableBelow : availableAbove;
    const menuHeight = Math.min(
      measuredHeight,
      availableHeight,
      window.innerHeight - viewportPadding * 2,
    );
    setPortalPosition({
      left: Math.max(
        viewportPadding,
        Math.min(triggerRect.right - menuWidth, window.innerWidth - menuWidth - viewportPadding),
      ),
      top: opensBelow
        ? Math.min(
            triggerRect.bottom + gap,
            window.innerHeight - menuHeight - viewportPadding,
          )
        : Math.max(viewportPadding, upperAnchor - menuHeight - gap),
      maxHeight: availableHeight,
    });
  }, [portal]);

  useLayoutEffect(() => {
    if (!open || !portal) return;
    updatePortalPosition();
    const frame = requestAnimationFrame(updatePortalPosition);
    window.addEventListener("resize", updatePortalPosition);
    window.addEventListener("scroll", updatePortalPosition, true);
    const observer = typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(updatePortalPosition);
    if (menuRef.current) observer?.observe(menuRef.current);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", updatePortalPosition);
      window.removeEventListener("scroll", updatePortalPosition, true);
      observer?.disconnect();
    };
  }, [open, portal, updatePortalPosition]);

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
  const policyLabel =
    activePolicy === "fixed" ? "固定" : activePolicy === "auto" ? "自动" : "首选";
  const showEndpoint = activeEndpoint !== (settings?.default_endpoint ?? activeEndpoint);
  const triggerLabel = `${showEndpoint ? `${activeEndpoint} / ` : ""}${displayed}${
    activePolicy === "prefer" ? "" : ` · ${policyLabel}`
  }`;

  const menu = (
      <div
        ref={menuRef}
        id={menuId}
        role="dialog"
        aria-label="选择下一回合模型"
        className="z-[100] w-72 max-w-[calc(100vw-1rem)] rounded-lg border border-border/50 bg-surface-2 shadow-lg"
      >
      <div className="space-y-2 border-b border-border/35 p-2">
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
              className="min-h-[44px] w-full rounded border border-control-border bg-surface-3 px-2 py-1 text-label text-gray-200 outline-none focus:border-accent focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
            >
              <option value="fixed">固定 · 只使用当前模型</option>
              <option value="prefer">首选 · 安全时允许兼容接管</option>
              <option value="auto">自动 · 按能力与状态选择</option>
            </select>
            <p className="px-0.5 text-label leading-5 text-gray-500">
              会话策略更改只从下一轮开始生效；当前运行中的回合不会改路。
            </p>
            <ReasoningEffortPicker embedded />
            <ReleaseUrgencyPicker />
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
            className="min-h-[44px] w-full rounded border border-control-border bg-surface-3 px-2 py-1 text-label text-gray-200 outline-none focus:border-accent focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
          >
            {endpointKeys.map((key) => (
              <option key={key} value={key}>{key}</option>
            ))}
          </select>
        )}
        <input
          autoFocus
          aria-label="搜索模型"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          disabled={modelListLoading}
          placeholder="搜索模型…"
          className="min-h-[44px] w-full rounded border border-control-border bg-surface-3 px-2 py-1 text-label text-gray-200 placeholder-gray-600 outline-none focus:border-accent focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50 lg:min-h-[36px]"
        />
        {activeSession && sessionOverrides.includes(activeSession.id) && (
          <button
            type="button"
            className="mt-2 min-h-[44px] w-full rounded border border-accent/30 bg-accent/5 px-2 py-1 text-left text-label text-gray-300 transition-colors hover:bg-accent/10 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
            onClick={async () => {
              await invoke("set_session_usage_window_override", {
                sessionId: activeSession.id,
                ignore: false,
              });
              setSessionOverrides((previous) => previous.filter((id) => id !== activeSession.id));
            }}
          >
            本会话已忽略时段限制 · 恢复本会话的时段限制
          </button>
        )}
      </div>
      <ul className="max-h-64 overflow-y-auto py-1">
        {modelListLoading ? (
          <li className="px-3 py-2 text-label text-gray-600">正在加载模型…</li>
        ) : filtered.slice(0, 50).map((m) => (
          <li key={m.id}>
            <button
              className={`flex min-h-[44px] w-full items-center gap-1.5 px-3 py-1.5 text-left text-label transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent lg:min-h-[36px] ${
                m.id === activeModel ? "text-accent" : "text-gray-300"
              }`}
              onClick={() => {
                const rule = settings?.model_usage_windows?.[modelWindowKey(activeEndpoint, m.id)] ?? settings?.endpoint_usage_windows?.[activeEndpoint];
                const status = evaluateWindow(rule, Date.now());
                if (!status.available) {
                  // CF-WINDOW-R6：不静默改用别的模型，也不静默违反配置。说明
                  // 原因，并把一次性豁免交给应用内对话框决定。
                  setWindowPrompt({
                    endpoint: activeEndpoint,
                    model: m.id,
                    nextTransitionMs: status.next_transition_ms,
                  });
                  return;
                }
                void selectModel(m.id);
              }}
              title={m.id}
            >
              {m.is_custom && <Sparkles size={14} className="shrink-0 text-amber-400" />}
              <span className="truncate">{m.id}</span>
            </button>
          </li>
        ))}
        {!modelListLoading && filtered.length === 0 && (
          <li className="px-3 py-2 text-label text-gray-600">未找到模型</li>
        )}
      </ul>
    </div>
  );

  // CF-WINDOW-R6：把"选中模型"抽出来，让对话框确认后能继续同一条路径。
  const selectModel = useCallback(async (modelId: string) => {
    if (activeSession) {
      await updateActiveSessionModelConfig({
        endpointId: activeEndpoint,
        modelId,
        policy: activePolicy,
      });
    } else {
      // Outside an existing session this remains the
      // new-session default for the endpoint.
      await invoke("set_endpoint_active_model", {
        endpointName: activeEndpoint,
        modelId,
      }).catch(() => { /* best-effort */ });
      await updateActiveSessionModel(modelId);
      await reloadSettings();
    }
    setOpen(false);
    triggerRef.current?.focus();
  }, [activeEndpoint, activePolicy, activeSession, reloadSettings, updateActiveSessionModel, updateActiveSessionModelConfig]);

  // 打开面板时读一次已持久化的会话豁免，这样重启后仍然能显示并提供撤销入口。
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    invoke<string[]>("get_session_usage_window_overrides")
      .then((overrides) => {
        if (!cancelled) setSessionOverrides(Array.isArray(overrides) ? overrides : []);
      })
      .catch(() => { /* the menu still works without the override list */ });
    return () => { cancelled = true; };
  }, [open]);

  const menuContent = portal && typeof document !== "undefined" ? (
    createPortal(
      <div
        data-testid="model-picker-portal-menu"
        className="fixed z-[100] overflow-y-auto"
        style={portalPosition}
      >
        {menu}
      </div>,
      document.body,
    )
  ) : (
    <div className="absolute right-0 top-full z-50 mt-1">{menu}</div>
  );

  return (
    <div ref={ref} className="relative min-w-0 max-w-full">
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={`flex min-h-[44px] min-w-0 max-w-full items-center gap-1 rounded-lg px-2 py-1 text-label transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px] ${
          prominent
            ? "max-w-full border border-accent/30 bg-accent/5 font-medium text-gray-200 hover:border-accent/60 hover:bg-accent/10"
            : "text-gray-400 hover:bg-surface-3 hover:text-gray-200"
        }`}
        aria-label={`选择下一回合模型：${activeEndpoint} / ${displayed} · ${policyLabel}`}
        aria-expanded={open}
        aria-controls={menuId}
        aria-haspopup="dialog"
        title={`${activeEndpoint} / ${activeModel} · ${activePolicy}`}
      >
        <span className="max-w-[116px] truncate sm:max-w-[190px]">
          {triggerLabel}
        </span>
        <ChevronDown size={14} />
      </button>

      {open && menuContent}

      {windowPrompt && (
        <UsageWindowDialog
          endpoint={windowPrompt.endpoint}
          model={windowPrompt.model}
          nextTransitionMs={windowPrompt.nextTransitionMs}
          onIgnoreForSession={activeSession ? async () => {
            const target = windowPrompt.model;
            await invoke("set_session_usage_window_override", {
              sessionId: activeSession.id,
              ignore: true,
            });
            setSessionOverrides((previous) =>
              previous.includes(activeSession.id) ? previous : [...previous, activeSession.id],
            );
            setWindowPrompt(null);
            await selectModel(target);
          } : null}
          onCancel={() => setWindowPrompt(null)}
        />
      )}
    </div>
  );
}
