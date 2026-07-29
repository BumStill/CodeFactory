// SPDX-License-Identifier: Apache-2.0
import React, { useEffect, useState } from "react";
import {
  ArrowLeft, Plus, Trash2, Eye, EyeOff, Check, AlertCircle, ChevronDown,
  RefreshCw, Download, Package, LogIn, LogOut, Sparkles, Github, ExternalLink,
  ArrowRight, UserRound, GitPullRequestArrow, Gauge, Puzzle, ShieldCheck,
  PanelTop,
} from "lucide-react";
import {
  invoke,
  codexLogout,
  codexAccount,
  codexLoginStart,
  codexLoginOpen,
  codexLoginStatus,
  codexLoginCancel,
  listBrowserSessions,
  closeBrowserSession,
} from "../../lib/tauri";
import { useSettingsStore } from "../../stores/settings";
import { useChatStore } from "../../stores/chat";
import { useGitRemoteStore } from "../../stores/gitRemote";
import { useUpdaterStore, type UpdaterPhase } from "../../stores/updater";
import type {
  Settings,
  Endpoint,
  ApiStyle,
  CustomModel,
  AddGitRemoteRequest,
  GitRemoteConfig,
  GitProvider,
  CodexAccount,
  CodexLoginFlow,
  GithubCliCredentialStatus,
  BrowserSession,
} from "../../lib/tauri";
import { CHATGPT_DEFAULT_MODEL, CHATGPT_ENDPOINT_KEY } from "../../lib/chatgptModels";
import { syncChatGptCatalog } from "../../stores/chatgptCatalog";
import { UsageDashboardSection } from "../../components/UsageDashboardSection";

export type SettingsTab = "capabilities" | "usage" | "endpoints" | "browser" | "general" | "hooks" | "remotes" | "appearance" | "about";

interface Props {
  onBack: () => void;
  initialTab?: SettingsTab;
  onOpenSession?: (sessionId: string) => void;
  onOpenJobLog?: (sessionId: string, taskId: string) => void;
  onOpenProfile?: () => void;
  onOpenEvolution?: () => void;
  onOpenBenchmarks?: () => void;
  onOpenResources?: () => void;
  onOpenControlPlane?: () => void;
}

type Tab = SettingsTab;
const SHELL_OPTIONS =
  typeof navigator !== "undefined" && /Mac|Linux/.test(navigator.platform)
    ? ["zsh", "bash", "powershell", "cmd"]
    : ["powershell", "cmd", "bash"];

// ── Hooks types ───────────────────────────────────────────────────────────────

type HookActionType = "log_to_file" | "run_command" | "emit_event" | "auto_git_commit";

interface HookConfig {
  id: string;
  name: string;
  event: string;
  action: {
    type: HookActionType;
    path?: string;
    command?: string;
    cwd?: string;
    event_name?: string;
    message_template?: string;
  };
  enabled: boolean;
  filter: string | null;
}

// ── Helpers ───────────────────────────────────────────────────────────────────

interface EndpointDraft {
  key: string;       // map key in settings.endpoints
  base_url: string;
  api_style: ApiStyle;
  api_key: string;   // replacement-only input; saved key values are not read by default
  key_ref?: string;  // e.g. "codefactory.endpoint.myname"
  custom_models: CustomModel[];
}

// ── Custom Models sub-editor (nested in EndpointCard) ────────────────────────

function CustomModelsEditor({
  models,
  onChange,
}: {
  models: CustomModel[];
  onChange: (next: CustomModel[]) => void;
}) {
  const [expanded, setExpanded] = useState(models.length > 0);

  const updateAt = (idx: number, patch: Partial<CustomModel>) =>
    onChange(models.map((m, i) => (i === idx ? { ...m, ...patch } : m)));

  const removeAt = (idx: number) =>
    onChange(models.filter((_, i) => i !== idx));

  const addNew = () =>
    onChange([...models, { id: "", name: "" }]);

  return (
    <div className="space-y-1.5 pt-1 border-t border-border/60">
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="flex w-full items-center justify-between text-[11px] text-gray-500 hover:text-gray-300"
      >
        <span>
          自定义模型
          {models.length > 0 && (
            <span className="ml-1.5 text-[10px] text-gray-600">({models.length})</span>
          )}
        </span>
        <ChevronDown
          size={11}
          className={`transition-transform ${expanded ? "rotate-180" : ""}`}
        />
      </button>

      {expanded && (
        <div className="space-y-1.5">
          {models.length === 0 && (
            <div className="text-[11px] text-gray-600 italic">
              暂无自定义模型。适用于不暴露 <code>/models</code> 的 LMStudio / Ollama / 私有网关。
            </div>
          )}

          {models.map((m, idx) => (
            <div key={idx} className="flex items-center gap-1.5">
              <input
                value={m.id}
                onChange={(e) => updateAt(idx, { id: e.target.value })}
                placeholder="模型 ID(例如 llama3.1:8b)"
                className="flex-[2] bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
              />
              <input
                value={m.name ?? ""}
                onChange={(e) =>
                  updateAt(idx, { name: e.target.value || undefined })
                }
                placeholder="显示名称(可选)"
                className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
              />
              <button
                onClick={() => removeAt(idx)}
                className="p-1 text-gray-600 hover:text-red-400 transition-colors"
                title="移除"
              >
                <Trash2 size={12} />
              </button>
            </div>
          ))}

          <button
            type="button"
            onClick={addNew}
            className="flex items-center gap-1 text-[11px] text-accent hover:text-accent-hover"
          >
            <Plus size={11} /> 添加模型
          </button>
        </div>
      )}
    </div>
  );
}

function EndpointCard({
  draft,
  isDefault,
  onSetDefault,
  onSave,
  onDelete,
}: {
  draft: EndpointDraft;
  isDefault: boolean;
  onSetDefault: () => void;
  onSave: (d: EndpointDraft) => Promise<void>;
  onDelete: () => void;
}) {
  const [local, setLocal] = useState(draft);
  const [showKey, setShowKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const hasSavedKeyRef = Boolean(local.key_ref);

  useEffect(() => {
    setLocal(draft);
    setShowKey(false);
  }, [draft]);

  const dirty =
    local.base_url !== draft.base_url ||
    local.api_style !== draft.api_style ||
    local.api_key !== draft.api_key ||
    JSON.stringify(local.custom_models) !== JSON.stringify(draft.custom_models);

  const handleSave = async () => {
    setSaving(true);
    await onSave(local);
    setSaving(false);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };

  return (
    <div
      className={`rounded-lg border p-3 space-y-2.5 ${
        isDefault ? "border-accent/40 bg-surface-2" : "border-border bg-surface-1"
      }`}
    >
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium text-gray-200">{local.key}</span>
        <div className="flex items-center gap-2">
          {isDefault ? (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-accent/20 text-accent">
              默认
            </span>
          ) : (
            <button
              onClick={onSetDefault}
              className="text-[10px] px-1.5 py-0.5 rounded border border-border text-gray-500 hover:text-gray-300 hover:border-gray-500 transition-colors"
            >
              设为默认
            </button>
          )}
          <button
            onClick={onDelete}
            className="p-0.5 text-gray-600 hover:text-red-400 transition-colors"
          >
            <Trash2 size={12} />
          </button>
        </div>
      </div>

      {/* Base URL */}
      <div className="space-y-1">
        <label className="text-[11px] text-gray-500">基础 URL</label>
        <input
          value={local.base_url}
          onChange={(e) => setLocal({ ...local, base_url: e.target.value })}
          placeholder="https://openrouter.ai/api/v1"
          className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
        />
      </div>

      {/* API Key */}
      <div className="space-y-1">
        <label className="text-[11px] text-gray-500">API 密钥</label>
        <div className="flex gap-1">
          <input
            type={showKey ? "text" : "password"}
            value={local.api_key}
            onChange={(e) => setLocal({ ...local, api_key: e.target.value })}
            placeholder={hasSavedKeyRef ? "已保存，输入新密钥以替换" : "sk-…"}
            className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
          />
          <button
            onClick={() => setShowKey((v) => !v)}
            disabled={local.api_key.length === 0}
            className="p-1 rounded border border-border text-gray-500 hover:text-gray-300 disabled:opacity-40 disabled:hover:text-gray-500"
            title="显示/隐藏本次输入的密钥"
          >
            {showKey ? <EyeOff size={12} /> : <Eye size={12} />}
          </button>
        </div>
        <p className="text-[11px] leading-4 text-gray-600">
          {hasSavedKeyRef
            ? "已保存。macOS 同时保留权限为 0600 的本机可用性副本，避免每次使用都弹出密钥授权；输入新密钥可替换。"
            : "保存时写入系统凭据库；macOS 另存权限为 0600 的本机可用性副本。两者都不进入设置备份。"}
        </p>
      </div>

      {/* API Style */}
      <div className="space-y-1">
        <label className="text-[11px] text-gray-500">API 风格</label>
        <div className="flex gap-3">
          {(["openai", "anthropic"] as ApiStyle[]).map((style) => (
            <label key={style} className="flex items-center gap-1.5 cursor-pointer">
              <input
                type="radio"
                name={`style-${local.key}`}
                checked={local.api_style === style}
                onChange={() => setLocal({ ...local, api_style: style })}
              />
              <span className="text-xs text-gray-300">
                {style === "openai" ? "OpenAI 兼容" : "Anthropic Messages"}
              </span>
            </label>
          ))}
        </div>
      </div>

      {/* Custom Models */}
      <CustomModelsEditor
        models={local.custom_models}
        onChange={(custom_models) => setLocal({ ...local, custom_models })}
      />

      {dirty && (
        <div className="flex justify-end">
          <button
            onClick={handleSave}
            disabled={saving}
            className="flex items-center gap-1 px-3 py-1 rounded bg-accent hover:bg-accent-hover text-xs text-white transition-colors disabled:opacity-50"
          >
            {saved ? <><Check size={11} /> 已保存</> : saving ? "保存中…" : "保存"}
          </button>
        </div>
      )}
    </div>
  );
}

// ── Add Endpoint dialog ───────────────────────────────────────────────────────

function AddEndpointModal({
  existing,
  onAdd,
  onClose,
}: {
  existing: string[];
  onAdd: (key: string, ep: Endpoint) => void;
  onClose: () => void;
}) {
  const [key, setKey] = useState("");
  const [url, setUrl] = useState("");
  const [style, setStyle] = useState<ApiStyle>("openai");
  const [err, setErr] = useState("");

  const handleAdd = () => {
    const k = key.trim().toLowerCase().replace(/\s+/g, "-");
    if (!k) { setErr("请填写名称"); return; }
    if (existing.includes(k)) { setErr(`"${k}" 已存在`); return; }
    if (!url.trim()) { setErr("请填写基础 URL"); return; }
    onAdd(k, { base_url: url.trim(), api_style: style, custom_models: [] });
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        className="w-96 rounded-xl border border-border bg-surface-2 shadow-2xl p-4 space-y-3"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-sm font-semibold text-gray-200">添加端点</h3>

        <div className="space-y-1">
          <label className="text-[11px] text-gray-500">名称(slug)</label>
          <input
            autoFocus
            value={key}
            onChange={(e) => { setKey(e.target.value); setErr(""); }}
            placeholder="我的端点"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-gray-500">基础 URL</label>
          <input
            value={url}
            onChange={(e) => { setUrl(e.target.value); setErr(""); }}
            placeholder="https://api.example.com/v1"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-gray-500">API 风格</label>
          <div className="flex gap-3">
            {(["openai", "anthropic"] as ApiStyle[]).map((s) => (
              <label key={s} className="flex items-center gap-1.5 cursor-pointer">
                <input type="radio" checked={style === s} onChange={() => setStyle(s)} />
                <span className="text-xs text-gray-300">
                  {s === "openai" ? "OpenAI 兼容" : "Anthropic Messages"}
                </span>
              </label>
            ))}
          </div>
        </div>

        {err && (
          <div className="flex items-center gap-1 text-xs text-red-400">
            <AlertCircle size={12} /> {err}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-1">
          <button onClick={onClose} className="px-3 py-1.5 rounded text-xs text-gray-500 hover:text-gray-300">
            取消
          </button>
          <button onClick={handleAdd} className="px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-xs text-white transition-colors">
            添加
          </button>
        </div>
      </div>
    </div>
  );
}


// ── Main Settings Page ────────────────────────────────────────────────────────

export function SettingsPage({
  onBack,
  initialTab,
  onOpenSession,
  onOpenJobLog,
  onOpenProfile,
  onOpenEvolution,
  onOpenBenchmarks,
  onOpenResources,
  onOpenControlPlane,
}: Props) {
  const { settings, load, save, saveApiKey } = useSettingsStore();
  const [tab, setTab] = useState<Tab>((initialTab as Tab | undefined) ?? "endpoints");
  const [endpointDrafts, setEndpointDrafts] = useState<EndpointDraft[]>([]);
  const [showAddEp, setShowAddEp] = useState(false);
  const [generalDraft, setGeneralDraft] = useState<{
    default_model: string;
    shell: string;
    auto_create_pr: boolean;
    remote_postmortem_enabled: boolean;
    reasoning_effort: Settings["reasoning_effort"];
    max_parallel_tasks: number;
    subagent_isolation: NonNullable<Settings["subagent_isolation"]>;
    delivery_ceiling: NonNullable<Settings["delivery_ceiling"]>;
    im_webhook_url: string;
    im_webhook_format: NonNullable<Settings["im_webhook_format"]>;
    sandbox_mode: NonNullable<Settings["sandbox_mode"]>;
    sandbox_image: string;
  } | null>(null);
  const [generalSaved, setGeneralSaved] = useState(false);

  // ── Load ───────────────────────────────────────────────────────────────────

  useEffect(() => {
    load();
  }, []);

  useEffect(() => {
    if (!settings) return;

    // Build endpoint drafts without reading saved keychain values. Reading raw
    // API keys can trigger OS password prompts on macOS, and the settings page
    // only needs to know whether a key reference exists.
    setEndpointDrafts(
      Object.entries(settings.endpoints).map(([key, ep]) => ({
        key,
        base_url: ep.base_url,
        api_style: ep.api_style,
        api_key: "",
        key_ref: ep.key_ref,
        custom_models: ep.custom_models ?? [],
      })),
    );

    setGeneralDraft({
      default_model: settings.default_model,
      shell: settings.shell.shell,
      auto_create_pr: (settings as Settings & { auto_create_pr?: boolean }).auto_create_pr ?? false,
      remote_postmortem_enabled: settings.remote_postmortem_enabled ?? false,
      reasoning_effort: settings.reasoning_effort ?? "medium",
      max_parallel_tasks: settings.max_parallel_tasks ?? 3,
      subagent_isolation: settings.subagent_isolation ?? "shared",
      delivery_ceiling: settings.delivery_ceiling ?? "through_release",
      im_webhook_url: settings.im_webhook_url ?? "",
      im_webhook_format: settings.im_webhook_format ?? "wecom",
      sandbox_mode: settings.sandbox_mode ?? "off",
      sandbox_image: settings.sandbox_image ?? "ubuntu:24.04",
    });
  }, [settings]);

  if (!settings || !generalDraft) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-gray-600">
        正在加载设置…
      </div>
    );
  }

  // ── Endpoint handlers ──────────────────────────────────────────────────────

  const handleSaveEndpoint = async (draft: EndpointDraft) => {
    // Persist a replacement API key only when the user typed one. Existing
    // key_ref values are preserved, but saved keychain values are never read
    // back into the form.
    const replacementApiKey = draft.api_key.trim();
    const keyRef =
      draft.key_ref ?? (replacementApiKey ? `codefactory.endpoint.${draft.key}` : undefined);
    if (replacementApiKey && keyRef) {
      await saveApiKey(keyRef, replacementApiKey);
    }

    // Rebuild endpoints map.
    // Filter out blank rows before persisting so trailing empty "Add model"
    // entries don't pollute the model picker.
    const cleanedModels = draft.custom_models
      .map((m) => ({ ...m, id: m.id.trim() }))
      .filter((m) => m.id.length > 0);

    const newEndpoints: Record<string, Endpoint> = { ...settings.endpoints };
    newEndpoints[draft.key] = {
      base_url: draft.base_url,
      api_style: draft.api_style,
      ...(keyRef ? { key_ref: keyRef } : {}),
      custom_models: cleanedModels,
    };
    await save({ ...settings, endpoints: newEndpoints });

    // Tell the chat store to reload models so the picker reflects the change
    // immediately — without waiting for endpoint switch.
    const chatState = useChatStore.getState();
    chatState.loadModels(settings.default_endpoint);

    // Update local drafts state with the cleaned (id-trimmed, blank-stripped) model list
    setEndpointDrafts((prev) =>
      prev.map((d) =>
        d.key === draft.key
          ? { ...draft, api_key: "", key_ref: keyRef, custom_models: cleanedModels }
          : d,
      ),
    );
  };

  const handleSetDefault = async (key: string) => {
    await save({ ...settings, default_endpoint: key });
  };

  const handleDeleteEndpoint = async (key: string) => {
    if (settings.default_endpoint === key) return; // can't delete default
    const keyRef =
      settings.endpoints[key]?.key_ref ?? `codefactory.endpoint.${key}`;
    await invoke("delete_api_key", { keyRef });
    const newEndpoints = { ...settings.endpoints };
    delete newEndpoints[key];
    await save({ ...settings, endpoints: newEndpoints });
    setEndpointDrafts((prev) => prev.filter((d) => d.key !== key));
  };

  const handleAddEndpoint = async (key: string, ep: Endpoint) => {
    const newEndpoints = { ...settings.endpoints, [key]: ep };
    await save({ ...settings, endpoints: newEndpoints });
    setEndpointDrafts((prev) => [
      ...prev,
      {
        key,
        base_url: ep.base_url,
        api_style: ep.api_style,
        api_key: "",
        key_ref: ep.key_ref,
        custom_models: ep.custom_models ?? [],
      },
    ]);
  };

  // ── General handlers ───────────────────────────────────────────────────────

  const handleSaveGeneral = async () => {
    if (!generalDraft) return;

    // We dropped the "Default Model" UI control; model selection is fully
    // per-endpoint now (see ModelPicker in the chat header). Only shell +
    // auto_create_pr remain on this tab. Keep default_model untouched in
    // settings.json — it stays as a back-compat fallback for Settings::active_model_for.
    await save({
      ...settings,
      shell: { shell: generalDraft.shell },
      auto_create_pr: generalDraft.auto_create_pr,
      remote_postmortem_enabled: generalDraft.remote_postmortem_enabled,
      reasoning_effort: generalDraft.reasoning_effort,
      max_parallel_tasks: Math.min(8, Math.max(1, Math.round(generalDraft.max_parallel_tasks) || 3)),
      subagent_isolation: generalDraft.subagent_isolation,
      delivery_ceiling: generalDraft.delivery_ceiling,
      delivery_ceiling_explicit: true,
      im_webhook_url: generalDraft.im_webhook_url.trim(),
      im_webhook_format: generalDraft.im_webhook_format,
      sandbox_mode: generalDraft.sandbox_mode,
      sandbox_image: generalDraft.sandbox_image.trim() || "ubuntu:24.04",
    } as Settings & { auto_create_pr: boolean });

    setGeneralSaved(true);
    setTimeout(() => setGeneralSaved(false), 1500);
  };

  // ── Render ─────────────────────────────────────────────────────────────────

  const tabs: { id: Tab; label: string }[] = [
    { id: "capabilities", label: "功能" },
    { id: "usage", label: "用量与预算" },
    { id: "endpoints", label: "端点" },
    { id: "browser", label: "浏览器会话" },
    { id: "general", label: "通用" },
    { id: "appearance", label: "外观" },
    { id: "hooks", label: "钩子" },
    { id: "remotes", label: "远程仓库" },
    { id: "about", label: "关于" },
  ];

  return (
    <div className="flex h-full flex-col bg-surface-0 text-gray-200">
      {/* Header */}
      <header className="flex flex-wrap items-center gap-3 px-4 py-2.5 border-b border-border bg-surface-1 shrink-0">
        <button
          onClick={onBack}
          className="p-1 rounded text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          <ArrowLeft size={14} />
        </button>
        <span className="text-sm font-semibold">设置</span>

        {/* Tabs */}
        <div className="ml-4 flex max-w-full gap-1 overflow-x-auto">
          {tabs.map((t) => (
            <button
              key={t.id}
              onClick={() => setTab(t.id)}
              className={`px-3 py-1 rounded text-xs transition-colors ${
                tab === t.id
                  ? "bg-surface-3 text-gray-200"
                  : "text-gray-500 hover:text-gray-300 hover:bg-surface-2"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>
      </header>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-5">

        {/* ── Product capabilities moved out of the Workspace toolbar ── */}
        {tab === "capabilities" && (
          <section className="max-w-3xl space-y-4" aria-labelledby="settings-capabilities-title">
            <div>
              <h2 id="settings-capabilities-title" className="text-base font-semibold text-gray-100">功能</h2>
              <p className="mt-1 text-xs leading-5 text-gray-400">管理跨会话能力。当前会话的模型、Git 和检查点仍留在工作区顶栏。</p>
            </div>
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              {([
                { label: "我的画像", description: "查看偏好、长期记忆与可撤销建议", Icon: UserRound, action: onOpenProfile },
                { label: "进化审查", description: "审核会话学习结果与激活候选", Icon: GitPullRequestArrow, action: onOpenEvolution },
                { label: "能力评测", description: "查看 Evals、能力基线和验证记录", Icon: Gauge, action: onOpenBenchmarks },
                { label: "资源中心", description: "管理可复用知识、技能与连接器", Icon: Puzzle, action: onOpenResources },
                { label: "AI Coding OS", description: "检查本地控制平面与同步状态", Icon: ShieldCheck, action: onOpenControlPlane },
              ] as const).map(({ label, description, Icon, action }) => (
                <button
                  key={label}
                  type="button"
                  onClick={action}
                  disabled={!action}
                  className="group flex items-center gap-3 rounded-xl border border-border bg-surface-1 p-3 text-left transition-colors hover:border-accent/40 hover:bg-surface-2 focus:outline-none focus:ring-2 focus:ring-accent/50 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-surface-3 text-gray-300 group-hover:text-accent"><Icon size={16} /></span>
                  <span className="min-w-0 flex-1">
                    <span className="block text-sm font-medium text-gray-100">{label}</span>
                    <span className="mt-0.5 block text-[11px] leading-4 text-gray-400">{description}</span>
                  </span>
                  <ArrowRight size={13} className="shrink-0 text-gray-500 group-hover:text-accent" />
                </button>
              ))}
            </div>
            <div className="rounded-lg border border-border bg-surface-1 px-3 py-2.5 text-[11px] leading-5 text-gray-400">
              规范随当前代码库存在；任务计划与拆解由会话内部执行，不再提供独立工作台。
            </div>
          </section>
        )}

        {/* ── Usage & budgets ── */}
        {tab === "usage" && (
          <UsageDashboardSection onOpenSession={onOpenSession} onOpenJobLog={onOpenJobLog} />
        )}

        {/* ── Endpoints ── */}
        {tab === "endpoints" && (
          <div className="max-w-xl space-y-3">
            <ChatGptLoginCard />

            <div className="rounded-lg border border-border bg-surface-1 p-3">
              <label className="flex items-center justify-between gap-4">
                <span>
                  <span className="block text-sm text-gray-200">新会话默认策略</span>
                  <span className="mt-1 block text-xs leading-5 text-gray-500">
                    只影响之后创建的会话；已有会话继续使用自己的策略。
                  </span>
                </span>
                <select
                  aria-label="新会话默认策略"
                  value={settings.default_model_policy ?? "prefer"}
                  onChange={(event) => {
                    void save({
                      ...settings,
                      default_model_policy: event.target.value as
                        | "fixed"
                        | "prefer"
                        | "auto",
                    });
                  }}
                  className="shrink-0 rounded border border-border bg-surface-3 px-2 py-1.5 text-xs text-gray-200 outline-none focus:border-accent/50"
                >
                  <option value="fixed">固定</option>
                  <option value="prefer">首选</option>
                  <option value="auto">自动</option>
                </select>
              </label>
            </div>

            <div className="flex items-center justify-between mb-1">
              <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
                API 端点
              </h2>
              <button
                onClick={() => setShowAddEp(true)}
                className="flex items-center gap-1 px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors border border-border"
              >
                <Plus size={11} /> 添加端点
              </button>
            </div>

            {/* The ChatGPT (OAuth) endpoint is managed by ChatGptLoginCard
                above — hide it from the editable list so its base_url / API
                key / API-style fields aren't shown (and so a card "Save" can't
                clobber its api_style back to "openai"). */}
            {endpointDrafts
              .filter((d) => d.key !== CHATGPT_ENDPOINT_KEY)
              .map((d) => (
                <EndpointCard
                  key={d.key}
                  draft={d}
                  isDefault={settings.default_endpoint === d.key}
                  onSetDefault={() => handleSetDefault(d.key)}
                  onSave={handleSaveEndpoint}
                  onDelete={() => handleDeleteEndpoint(d.key)}
                />
              ))}

            {endpointDrafts.filter((d) => d.key !== CHATGPT_ENDPOINT_KEY).length === 0 && (
              <p className="text-xs text-gray-600">尚未配置自定义 API 端点。</p>
            )}

            {showAddEp && (
              <AddEndpointModal
                existing={endpointDrafts.map((d) => d.key)}
                onAdd={handleAddEndpoint}
                onClose={() => setShowAddEp(false)}
              />
            )}
          </div>
        )}

        {/* ── Managed browser sessions ── */}
        {tab === "browser" && <BrowserSessionsTab />}

        {/* ── General ── */}
        {tab === "general" && (
          <div className="max-w-xl space-y-4">
            <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
              通用
            </h2>

            <div className="rounded-lg border border-border bg-surface-1 px-3 py-2.5 text-[11px] text-gray-500 leading-5">
              <span className="text-gray-300 font-medium">当前模型</span> 现在按端点设置。
              在聊天顶栏的模型下拉菜单中选择——每个端点都会记住各自的选择。
              在 <span className="text-gray-300">端点</span> 标签页中管理端点。
            </div>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">Shell</label>
              <div className="flex gap-3">
                {SHELL_OPTIONS.map((s) => (
                  <label key={s} className="flex items-center gap-1.5 cursor-pointer">
                    <input
                      type="radio"
                      checked={generalDraft.shell === s}
                      onChange={() => setGeneralDraft({ ...generalDraft, shell: s })}
                    />
                    <span className="text-xs text-gray-300">{s}</span>
                  </label>
                ))}
              </div>
            </div>

            <label className="flex items-start gap-3 rounded-lg border border-border bg-surface-1 px-3 py-2.5 cursor-pointer">
              <input
                type="checkbox"
                checked={generalDraft.auto_create_pr}
                onChange={(e) =>
                  setGeneralDraft({ ...generalDraft, auto_create_pr: e.target.checked })
                }
                className="mt-0.5"
              />
              <span>
                <span className="block text-xs font-medium text-gray-200">实现完成后自动创建 PR</span>
                <span className="block text-xs leading-5 text-gray-500">
                  当规格实现成功完成时自动创建一个拉取请求。
                </span>
              </span>
            </label>

            <label className="flex items-start gap-3 rounded-lg border border-border bg-surface-1 px-3 py-2.5 cursor-pointer">
              <input
                type="checkbox"
                checked={generalDraft.remote_postmortem_enabled}
                onChange={(e) =>
                  setGeneralDraft({ ...generalDraft, remote_postmortem_enabled: e.target.checked })
                }
                className="mt-0.5"
              />
              <span>
                <span className="block text-xs font-medium text-gray-200">允许远程会话复盘（发送摘要到模型）</span>
                <span className="block text-xs leading-5 text-gray-500">
                  仅控制这一项：会话结束后把有限、脱敏的摘要发送到当前配置的模型以生成候选，默认关闭。本地跨会话挖掘与进化审查是确定性的、不发送任何数据，会话结束后始终自动运行——与此开关无关。
                </span>
              </span>
            </label>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">默认思考强度</label>
              <p className="text-[11px] leading-5 text-gray-600">
                适用于推理模型(ChatGPT / Codex)。每个会话可在聊天顶栏的模型行中单独覆盖。
              </p>
              <select
                value={generalDraft.reasoning_effort ?? "medium"}
                onChange={(e) =>
                  setGeneralDraft({
                    ...generalDraft,
                    reasoning_effort: e.target.value as Settings["reasoning_effort"],
                  })
                }
                className="rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
              >
                {([
                  ["minimal", "最低"],
                  ["low", "低"],
                  ["medium", "中"],
                  ["high", "高"],
                  ["xhigh", "超高"],
                  ["max", "最大"],
                ] as const).map(([v, label]) => (
                  <option key={v} value={v}>
                    {label}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">并行任务上限</label>
              <p className="text-[11px] leading-5 text-gray-600">
                任务分解后同时运行的子代理数量(1–8)。
              </p>
              <input
                type="number"
                min={1}
                max={8}
                value={generalDraft.max_parallel_tasks}
                onChange={(e) =>
                  setGeneralDraft({
                    ...generalDraft,
                    max_parallel_tasks: Number(e.target.value),
                  })
                }
                className="w-20 rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
              />
            </div>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">子代理磁盘隔离</label>
              <p className="text-[11px] leading-5 text-gray-600">
                worktree 模式下每个并行任务在独立的 git worktree
                中工作,验证通过后才把改动合并回项目目录;冲突时项目目录保持原样,任务改动保留在分支上。非
                git 项目自动回退到共享目录。
              </p>
              <select
                value={generalDraft.subagent_isolation}
                onChange={(e) =>
                  setGeneralDraft({
                    ...generalDraft,
                    subagent_isolation: e.target
                      .value as NonNullable<Settings["subagent_isolation"]>,
                  })
                }
                className="rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
              >
                <option value="shared">共享目录(默认)</option>
                <option value="worktree">Git worktree 隔离</option>
              </select>
            </div>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">自动交付上限</label>
              <p className="text-[11px] leading-5 text-gray-600">
                代码改动测试通过后,AI 自动把工作推进到哪一步为止。默认一路合并、发布上线；如果你想人工接管,可以把边界降到 PR 或 CI。合并/发布受远端分支保护与凭据权限约束；CodeFactory 会优先使用
                「远程仓库」令牌，也会自动复用已登录的 GitHub CLI。
              </p>
              <select
                value={generalDraft.delivery_ceiling}
                onChange={(e) =>
                  setGeneralDraft({
                    ...generalDraft,
                    delivery_ceiling: e.target
                      .value as NonNullable<Settings["delivery_ceiling"]>,
                  })
                }
                className="rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
              >
                <option value="off">关闭(不自动交付)</option>
                <option value="pr_only">提交 + 推送 + 开 PR</option>
                <option value="through_ci_green">…并等 CI 通过</option>
                <option value="through_merge">…并合并</option>
                <option value="through_release">…并发布上线(默认)</option>
              </select>
            </div>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">IM 通知(手机上掌握进度)</label>
              <p className="text-[11px] leading-5 text-gray-600">
                配置群机器人 Webhook 后,任务完成/失败、会话回合中断、工具等待批准时会推送一条消息。
                仅单向通知,不含任何令牌或代码内容;留空即关闭。
              </p>
              <div className="flex items-center gap-2">
                <select
                  value={generalDraft.im_webhook_format}
                  onChange={(e) =>
                    setGeneralDraft({
                      ...generalDraft,
                      im_webhook_format: e.target.value as NonNullable<Settings["im_webhook_format"]>,
                    })
                  }
                  className="rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
                >
                  <option value="wecom">企业微信</option>
                  <option value="feishu">飞书</option>
                  <option value="generic">通用 JSON</option>
                </select>
                <input
                  value={generalDraft.im_webhook_url}
                  onChange={(e) =>
                    setGeneralDraft({ ...generalDraft, im_webhook_url: e.target.value })
                  }
                  placeholder="https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=…"
                  className="flex-1 rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
                  aria-label="IM Webhook 地址"
                />
              </div>
            </div>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">命令沙箱</label>
              <p className="text-[11px] leading-5 text-gray-600">
                开启后,AI 执行的每条 shell 命令都在一次性 Docker
                容器中运行,只挂载当前项目目录——本机其它文件对命令不可见。需要本机已安装并启动
                Docker;项目目录需位于 Docker 文件共享范围内(默认包含用户目录)。
              </p>
              <div className="flex items-center gap-2">
                <select
                  value={generalDraft.sandbox_mode}
                  onChange={(e) =>
                    setGeneralDraft({
                      ...generalDraft,
                      sandbox_mode: e.target.value as NonNullable<Settings["sandbox_mode"]>,
                    })
                  }
                  className="rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
                >
                  <option value="off">关闭(本机执行,默认)</option>
                  <option value="docker">Docker 容器隔离</option>
                </select>
                {generalDraft.sandbox_mode === "docker" && (
                  <input
                    value={generalDraft.sandbox_image}
                    onChange={(e) =>
                      setGeneralDraft({ ...generalDraft, sandbox_image: e.target.value })
                    }
                    placeholder="ubuntu:24.04"
                    className="w-44 rounded border border-border bg-surface-2 px-2 py-1 text-xs text-gray-300"
                    aria-label="沙箱镜像"
                  />
                )}
              </div>
            </div>

            <div className="flex justify-end">
              <button
                onClick={handleSaveGeneral}
                className="flex items-center gap-1 px-4 py-1.5 rounded bg-accent hover:bg-accent-hover text-xs text-white transition-colors"
              >
                {generalSaved ? <><Check size={11} /> 已保存</> : "保存"}
              </button>
            </div>

            <DataSection />
          </div>
        )}

        {/* ── Appearance ── */}
        {tab === "appearance" && <AppearanceTab />}

        {/* ── Hooks ── */}
        {tab === "hooks" && <HooksTab />}

        {/* ── Remotes ── */}
        {tab === "remotes" && <RemotesTab />}

        {/* ── About ── */}
        {tab === "about" && <AboutTab />}

      </div>
    </div>
  );
}

// ── BrowserSessionsTab ───────────────────────────────────────────────────────

function BrowserSessionsTab() {
  const [sessions, setSessions] = useState<BrowserSession[]>([]);
  const [loading, setLoading] = useState(true);
  const [closing, setClosing] = useState<string | null>(null);
  const [error, setError] = useState("");

  const load = async () => {
    setLoading(true);
    setError("");
    try {
      setSessions(await listBrowserSessions());
    } catch (loadError) {
      setError(String(loadError));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const close = async (sessionId: string) => {
    setClosing(sessionId);
    setError("");
    try {
      await closeBrowserSession(sessionId);
      await load();
    } catch (closeError) {
      setError(String(closeError));
    } finally {
      setClosing(null);
    }
  };

  return (
    <section className="max-w-3xl space-y-4" aria-labelledby="browser-sessions-title">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 id="browser-sessions-title" className="text-base font-semibold text-gray-100">
            受管浏览器会话
          </h2>
          <p className="mt-1 text-xs leading-5 text-gray-400">
            这里只显示 CodeFactory 创建的自动化浏览器。结束会话不会关闭你的普通 Chrome 窗口。
          </p>
        </div>
        <button
          type="button"
          onClick={() => void load()}
          disabled={loading}
          className="flex items-center gap-1 rounded border border-border bg-surface-1 px-2.5 py-1.5 text-xs text-gray-400 hover:text-gray-200 disabled:opacity-50"
        >
          <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          刷新
        </button>
      </div>

      {error && (
        <div role="alert" className="rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-700 dark:text-red-300">
          {error}
        </div>
      )}

      {!loading && sessions.length === 0 && (
        <div className="flex items-center gap-3 rounded-xl border border-border bg-surface-1 p-4 text-sm text-gray-400">
          <PanelTop size={18} className="text-emerald-400" />
          当前没有活动的 CodeFactory 自动化浏览器。
        </div>
      )}

      <div className="space-y-2">
        {sessions.map((session) => (
          <div
            key={session.session_id}
            className="flex items-center gap-3 rounded-xl border border-border bg-surface-1 p-3"
          >
            <span
              aria-label={session.expired ? "租约已过期" : "会话活动中"}
              className={`h-2.5 w-2.5 shrink-0 rounded-full ${
                session.expired ? "bg-amber-400" : "bg-emerald-400"
              }`}
            />
            <div className="min-w-0 flex-1">
              <div className="truncate text-xs font-medium text-gray-200">
                {session.task_id ? `任务 ${session.task_id}` : `会话 ${session.owner_session_id ?? "未知"}`}
              </div>
              <div className="mt-1 truncate font-mono text-[10px] text-gray-500">
                {session.session_id}
              </div>
              <div className="mt-1 text-[10px] text-gray-500">
                最后活动：{new Date(session.updated_at_unix_secs * 1000).toLocaleString()}
              </div>
            </div>
            <button
              type="button"
              aria-label={`结束浏览器会话 ${session.session_id}`}
              onClick={() => void close(session.session_id)}
              disabled={closing === session.session_id}
              className="rounded border border-red-500/30 px-2.5 py-1.5 text-xs text-red-700 hover:bg-red-500/10 disabled:opacity-50 dark:text-red-300"
            >
              {closing === session.session_id ? "正在结束…" : "结束会话"}
            </button>
          </div>
        ))}
      </div>
    </section>
  );
}

// ── HooksTab ──────────────────────────────────────────────────────────────────

const HOOK_EVENTS = [
  "pre_tool", "post_tool", "pre_task", "post_task",
  "session_start", "session_end", "verification_failed",
];

const HOOK_ACTIONS: { value: HookActionType; label: string; placeholder: string }[] = [
  { value: "log_to_file",     label: "记录到文件",               placeholder: "C:\\logs\\codefactory.jsonl" },
  { value: "run_command",     label: "运行命令",                 placeholder: "echo hook fired" },
  { value: "emit_event",      label: "发送 Tauri 事件",          placeholder: "my-hook-event" },
  { value: "auto_git_commit", label: "自动 git 提交(post_task)", placeholder: "chore: {task_title}" },
];

function HooksTab() {
  const [hooks, setHooks] = useState<HookConfig[]>([]);
  const [addOpen, setAddOpen] = useState(false);
  const [testResult, setTestResult] = useState<{ id: string; result: string } | null>(null);

  const load = async () => {
    try { setHooks(await invoke<HookConfig[]>("list_hooks")); } catch {}
  };

  useEffect(() => { load(); }, []);

  const handleToggle = async (h: HookConfig) => {
    await invoke("update_hook", { id: h.id, config: { ...h, enabled: !h.enabled } });
    await load();
  };

  const handleDelete = async (id: string) => {
    await invoke("delete_hook", { id });
    await load();
  };

  const handleTest = async (id: string) => {
    try {
      const result = await invoke<string>("test_hook", { id });
      setTestResult({ id, result });
    } catch (e) {
      setTestResult({ id, result: String(e) });
    }
  };

  return (
    <div className="max-w-xl space-y-3">
      <div className="flex items-center justify-between">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">钩子</h2>
        <button
          onClick={() => setAddOpen(true)}
          aria-label="打开添加钩子表单"
          className="flex items-center gap-1 px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 border border-border transition-colors"
        >
          <Plus size={11} /> 添加钩子
        </button>
      </div>

      {hooks.length === 0 && <p className="text-xs text-gray-600">尚未配置钩子。</p>}

      {hooks.map((hook) => (
        <div key={hook.id} className="rounded-lg border border-border bg-surface-1 px-3 py-2 space-y-1.5">
          <div className="flex items-center gap-2">
            <span className="flex-1 text-xs font-medium text-gray-200 truncate">{hook.name}</span>
            <span className="text-[10px] bg-surface-3 text-gray-500 px-1.5 py-0.5 rounded">{hook.event}</span>
            <button
              onClick={() => handleToggle(hook)}
              aria-label={`${hook.enabled ? "禁用" : "启用"}钩子 ${hook.name}`}
              className={`text-[10px] px-1.5 py-0.5 rounded transition-colors ${
                hook.enabled ? "bg-accent/20 text-accent" : "bg-surface-3 text-gray-600"
              }`}
            >
              {hook.enabled ? "开" : "关"}
            </button>
            <button
              onClick={() => handleTest(hook.id)}
              aria-label={`测试钩子 ${hook.name}`}
              className="text-[10px] text-gray-600 hover:text-gray-300 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors"
            >
              测试
            </button>
            <button
              onClick={() => handleDelete(hook.id)}
              aria-label={`删除钩子 ${hook.name}`}
              className="text-[10px] text-red-700 hover:text-red-400 px-1.5 py-0.5 rounded transition-colors"
            >
              <Trash2 size={10} />
            </button>
          </div>
          <div className="text-[10px] text-gray-600 font-mono truncate">
            {hook.action.type}: {hook.action.path ?? hook.action.command ?? hook.action.event_name ?? hook.action.message_template ?? ""}
          </div>
          {testResult?.id === hook.id && (
            <pre className="text-[10px] text-gray-400 bg-surface-3 rounded p-1.5 whitespace-pre-wrap max-h-20 overflow-y-auto">
              {testResult.result}
            </pre>
          )}
        </div>
      ))}

      {addOpen && <AddHookForm onAdded={() => { load(); setAddOpen(false); }} onCancel={() => setAddOpen(false)} />}
    </div>
  );
}

function AddHookForm({ onAdded, onCancel }: { onAdded: () => void; onCancel: () => void }) {
  const [name, setName]             = useState("");
  const [event, setEvent]           = useState("post_tool");
  const [actionType, setActionType] = useState<HookActionType>("log_to_file");
  const [actionParam, setActionParam] = useState("");
  const [filter, setFilter]         = useState("");
  const [saving, setSaving]         = useState(false);
  const [err, setErr]               = useState<string | null>(null);

  const currentAction = HOOK_ACTIONS.find((a) => a.value === actionType);

  const buildAction = () => {
    switch (actionType) {
      case "log_to_file":     return { type: "log_to_file" as const,     path: actionParam };
      case "run_command":     return { type: "run_command" as const,     command: actionParam, cwd: null };
      case "emit_event":      return { type: "emit_event" as const,      event_name: actionParam };
      case "auto_git_commit": return { type: "auto_git_commit" as const, message_template: actionParam };
    }
  };

  const handleSave = async () => {
    if (!name.trim() || !actionParam.trim()) { setErr("请填写名称和动作参数。"); return; }
    setSaving(true); setErr(null);
    try {
      await invoke("add_hook", {
        config: {
          id: `hook-${Date.now()}`,
          name: name.trim(), event,
          action: buildAction(),
          enabled: true,
          filter: filter.trim() || null,
        },
      });
      onAdded();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="rounded-lg border border-accent/30 bg-surface-1 p-3 space-y-2.5">
      <p className="text-xs font-medium text-gray-300">新建钩子</p>
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">名称</label>
          <input aria-label="名称" value={name} onChange={(e) => setName(e.target.value)} placeholder="我的钩子"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">事件</label>
          <select aria-label="事件" value={event} onChange={(e) => setEvent(e.target.value)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none">
            {HOOK_EVENTS.map((ev) => <option key={ev} value={ev}>{ev}</option>)}
          </select>
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">动作类型</label>
          <select aria-label="动作类型" value={actionType} onChange={(e) => setActionType(e.target.value as HookActionType)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none">
            {HOOK_ACTIONS.map((a) => <option key={a.value} value={a.value}>{a.label}</option>)}
          </select>
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">过滤器(可选)</label>
          <input aria-label="过滤器(可选)" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="例如 bash"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">{currentAction?.label ?? "参数"}</label>
          <input aria-label={currentAction?.label ?? "参数"} value={actionParam} onChange={(e) => setActionParam(e.target.value)}
            placeholder={currentAction?.placeholder ?? ""}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
      </div>
      {err && <p className="text-xs text-red-400">{err}</p>}
      <div className="flex justify-end gap-2">
        <button onClick={onCancel} className="px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300">取消</button>
        <button onClick={handleSave} disabled={saving}
          className="px-2 py-1 rounded bg-accent hover:bg-accent-hover text-xs text-white disabled:opacity-50 transition-colors">
          {saving ? "添加中…" : "添加钩子"}
        </button>
      </div>
    </div>
  );
}

// ── RemotesTab ────────────────────────────────────────────────────────────────

function RemotesTab() {
  const { remotes, loadRemotes, addRemote, deleteRemote, testRemote } = useGitRemoteStore();
  const [addOpen, setAddOpen]       = useState(false);
  const [testResults, setTestResults] = useState<Record<string, string>>({});
  const [testing, setTesting]       = useState<string | null>(null);
  const [githubCli, setGithubCli] = useState<GithubCliCredentialStatus | null>(null);

  useEffect(() => {
    loadRemotes();
    invoke<GithubCliCredentialStatus>("github_cli_credential_status")
      .then(setGithubCli)
      .catch(() => setGithubCli({ installed: false, authenticated: false }));
  }, [loadRemotes]);

  const handleTest = async (id: string) => {
    setTesting(id);
    try {
      const username = await testRemote(id);
      setTestResults((r) => ({ ...r, [id]: `✓ @${username}` }));
    } catch (e) {
      setTestResults((r) => ({ ...r, [id]: `✗ ${String(e)}` }));
    } finally {
      setTesting(null);
    }
  };

  return (
    <div className="max-w-xl space-y-3">
      <div className="flex items-center justify-between">
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          Git 远程仓库(GitHub / GitLab)
        </h2>
        <button
          onClick={() => setAddOpen(true)}
          aria-label="打开添加远程仓库表单"
          className="flex items-center gap-1 px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 border border-border transition-colors"
        >
          <Plus size={11} /> 添加远程仓库
        </button>
      </div>

      <div className="rounded-lg border border-border bg-surface-1 px-3 py-2 text-xs">
        {githubCli?.authenticated ? (
          <p className="text-green-400">✓ 已登录 GitHub CLI；PR 交付会自动复用该凭据，无需重复配置 token。</p>
        ) : githubCli?.installed ? (
          <p className="text-amber-400">GitHub CLI 尚未登录；运行 gh auth login，或添加远程仓库令牌。</p>
        ) : (
          <p className="text-gray-500">可添加远程仓库令牌；安装并登录 GitHub CLI 后也会被自动识别。</p>
        )}
      </div>

      {remotes.length === 0 && githubCli && !githubCli.authenticated && (
        <p className="text-xs text-gray-600">尚无可用的远程仓库凭据。</p>
      )}

      {remotes.map((remote: GitRemoteConfig) => (
        <div key={remote.id} className="rounded-lg border border-border bg-surface-1 px-3 py-2 space-y-1.5">
          <div className="flex items-center gap-2">
            <span className={`text-[9px] px-1.5 py-0.5 rounded font-medium ${
              remote.provider === "github" ? "bg-gray-700 text-gray-200" : "bg-orange-900 text-orange-200"
            }`}>
              {remote.provider}
            </span>
            <span className="flex-1 text-xs font-medium text-gray-200 truncate">{remote.name}</span>
            {remote.default_repo && (
              <span className="text-[10px] text-gray-600 font-mono truncate max-w-[120px]">
                {remote.default_repo}
              </span>
            )}
            <button
              onClick={() => handleTest(remote.id)}
              disabled={testing === remote.id}
              aria-label={`测试远程仓库 ${remote.name}`}
              className="text-[10px] text-gray-600 hover:text-gray-300 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors disabled:opacity-50"
            >
              {testing === remote.id ? "…" : "测试"}
            </button>
            <button
              onClick={() => deleteRemote(remote.id)}
              aria-label={`删除远程仓库 ${remote.name}`}
              className="text-[10px] text-red-700 hover:text-red-400 px-1.5 py-0.5 rounded transition-colors"
            >
              <Trash2 size={10} />
            </button>
          </div>
          {testResults[remote.id] && (
            <div className={`text-[10px] px-1 ${testResults[remote.id].startsWith("✓") ? "text-green-400" : "text-red-400"}`}>
              {testResults[remote.id]}
            </div>
          )}
        </div>
      ))}

      {addOpen && (
        <AddRemoteForm
          onAdded={() => { loadRemotes(); setAddOpen(false); }}
          onCancel={() => setAddOpen(false)}
          addRemote={addRemote}
        />
      )}
    </div>
  );
}

function AddRemoteForm({
  onAdded, onCancel, addRemote,
}: {
  onAdded: () => void;
  onCancel: () => void;
  addRemote: (config: AddGitRemoteRequest) => Promise<void>;
}) {
  const [name, setName]             = useState("");
  const [provider, setProvider]     = useState<GitProvider>("github");
  const [baseUrl, setBaseUrl]       = useState("https://api.github.com");
  const [token, setToken]           = useState("");
  const [showToken, setShowToken]   = useState(false);
  const [defaultRepo, setDefaultRepo] = useState("");
  const [saving, setSaving]         = useState(false);
  const [err, setErr]               = useState<string | null>(null);

  const handleProviderChange = (p: GitProvider) => {
    setProvider(p);
    setBaseUrl(p === "github" ? "https://api.github.com" : "https://gitlab.com/api/v4");
  };

  const handleSave = async () => {
    if (!name.trim() || !token.trim()) {
      setErr(provider === "github"
        ? "请填写名称和令牌；若已登录 GitHub CLI，则无需新增此配置。"
        : "请填写名称和令牌。");
      return;
    }
    setSaving(true); setErr(null);
    try {
      await addRemote({
        name: name.trim(), provider,
        base_url: baseUrl.trim(), token: token.trim(),
        default_repo: defaultRepo.trim() || null,
      });
      onAdded();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="rounded-lg border border-accent/30 bg-surface-1 p-3 space-y-2.5">
      <p className="text-xs font-medium text-gray-300">新建远程仓库</p>
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">名称</label>
          <input aria-label="名称" value={name} onChange={(e) => setName(e.target.value)} placeholder="我的 GitHub"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">提供商</label>
          <select aria-label="提供商" value={provider} onChange={(e) => handleProviderChange(e.target.value as GitProvider)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none">
            <option value="github">GitHub</option>
            <option value="gitlab">GitLab</option>
          </select>
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">基础 URL</label>
          <input aria-label="基础 URL" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none focus:border-accent/40" />
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">个人访问令牌</label>
          <div className="flex gap-1">
            <input aria-label="个人访问令牌" type={showToken ? "text" : "password"} value={token} onChange={(e) => setToken(e.target.value)}
              placeholder="ghp_…"
              className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
            <button onClick={() => setShowToken((v) => !v)}
              className="p-1 rounded border border-border text-gray-500 hover:text-gray-300">
              {showToken ? <EyeOff size={12} /> : <Eye size={12} />}
            </button>
          </div>
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">默认仓库(可选)</label>
          <input aria-label="默认仓库(可选)" value={defaultRepo} onChange={(e) => setDefaultRepo(e.target.value)} placeholder="owner/repo"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
      </div>
      {err && <p className="text-xs text-red-400">{err}</p>}
      <div className="flex justify-end gap-2">
        <button onClick={onCancel} className="px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300">取消</button>
        <button onClick={handleSave} disabled={saving}
          className="px-2 py-1 rounded bg-accent hover:bg-accent-hover text-xs text-white disabled:opacity-50 transition-colors">
          {saving ? "添加中…" : "添加远程仓库"}
        </button>
      </div>
    </div>
  );
}

// ── AppearanceTab — theme / font / font-size ─────────────────────────────────

import { Moon, Sun, Monitor } from "lucide-react";
import {
  FONT_FAMILIES,
  FONT_FAMILY_LABELS,
  FONT_SIZE_MIN,
  FONT_SIZE_MAX,
} from "../../stores/settings";
import type { Theme } from "../../lib/tauri";

function AppearanceTab() {
  const { settings, setTheme, setFontFamily, setFontSize } = useSettingsStore();
  if (!settings) return null;

  const themeOptions: { value: Theme; Icon: React.ElementType; label: string }[] = [
    { value: "dark",   Icon: Moon,    label: "深色" },
    { value: "light",  Icon: Sun,     label: "浅色" },
    { value: "system", Icon: Monitor, label: "跟随系统" },
  ];

  return (
    <div className="max-w-sm space-y-8">

      {/* Theme */}
      <div>
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-3">主题</h2>
        <div className="flex gap-2">
          {themeOptions.map(({ value, Icon, label }) => (
            <button
              key={value}
              onClick={() => setTheme(value)}
              className={`flex flex-col items-center gap-1.5 px-4 py-3 rounded-lg border transition-colors text-xs font-medium flex-1 ${
                settings.theme === value
                  ? "border-accent bg-surface-3 text-accent"
                  : "border-border bg-surface-2 text-gray-400 hover:border-gray-500 hover:text-gray-300"
              }`}
            >
              <Icon size={16} />
              {label}
            </button>
          ))}
        </div>
      </div>

      {/* Font family */}
      <div>
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-3">字体</h2>
        <div className="flex flex-col gap-2">
          {Object.entries(FONT_FAMILY_LABELS).map(([key, label]) => (
            <button
              key={key}
              onClick={() => setFontFamily(key)}
              className={`flex items-center justify-between px-3 py-2 rounded-lg border text-xs transition-colors ${
                settings.font_family === key
                  ? "border-accent bg-surface-3 text-gray-200"
                  : "border-border bg-surface-2 text-gray-400 hover:border-gray-500 hover:text-gray-300"
              }`}
            >
              <span>{label}</span>
              <span
                className="text-gray-500"
                style={{ fontFamily: FONT_FAMILIES[key] }}
              >
                Aa Bb Cc
              </span>
            </button>
          ))}
        </div>
      </div>

      {/* Font size */}
      <div>
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider mb-3">
          字号 <span className="text-gray-300 font-mono normal-case font-normal ml-1">{settings.font_size}px</span>
        </h2>
        <div className="flex items-center gap-3">
          <span className="text-xs text-gray-500 w-6 text-right">{FONT_SIZE_MIN}</span>
          <input
            type="range"
            min={FONT_SIZE_MIN}
            max={FONT_SIZE_MAX}
            step={1}
            value={settings.font_size}
            onChange={(e) => setFontSize(Number(e.target.value))}
            className="flex-1 accent-accent"
          />
          <span className="text-xs text-gray-500 w-6">{FONT_SIZE_MAX}</span>
        </div>
        <p className="mt-2 text-xs text-gray-500" style={{ fontSize: settings.font_size }}>
          预览：这是 {settings.font_size}px 的正文文字效果
        </p>
      </div>

    </div>
  );
}

// ── ChatGptLoginCard — "Sign in with ChatGPT" (Codex OAuth) ──────────────────
// Stage-1/3 surface: runs the OAuth login and shows the signed-in account.
// Wiring the signed-in session into model requests (subscription Responses API)
// is handled separately by the request layer.
function ChatGptLoginCard() {
  // undefined = still checking; null = signed out; object = signed in.
  const [account, setAccount] = useState<CodexAccount | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [flow, setFlow] = useState<CodexLoginFlow | null>(null);
  const [copied, setCopied] = useState(false);
  // Is ChatGPT the endpoint requests currently route to? Shown explicitly so
  // it's clear whether the subscription or one of the API endpoints is active.
  const isDefault =
    useSettingsStore((s) => s.settings?.default_endpoint) === CHATGPT_ENDPOINT_KEY;

  useEffect(() => {
    codexAccount()
      .then(async (a) => {
        setAccount(a);
        // Already signed in (e.g. from a prior session)? Make sure the ChatGPT
        // endpoint exists so the account is actually usable. Idempotent.
        if (a) await syncChatGptCatalog(true);
      })
      .catch(() => setAccount(null));
  }, []);

  useEffect(() => {
    if (!flow || (flow.status !== "waiting" && flow.status !== "exchanging")) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const next = await codexLoginStatus(flow.flow_id);
        if (cancelled) return;
        setFlow(next);
        if (next.status === "succeeded" && next.account) {
          setAccount(next.account);
          await syncChatGptCatalog(true);
        } else if (next.status === "failed") {
          setError(next.error_message ?? "ChatGPT 验证失败，请重试");
        }
      } catch (pollError) {
        if (!cancelled) {
          setError(pollError instanceof Error ? pollError.message : String(pollError));
        }
      }
    };
    const timer = window.setInterval(() => void poll(), 800);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [flow?.flow_id, flow?.status]);

  const handleLogin = async () => {
    setBusy(true);
    setError(null);
    try {
      setFlow(await codexLoginStart());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleOpenLogin = async () => {
    if (!flow) return;
    setError(null);
    try {
      const next = await codexLoginOpen(flow.flow_id);
      setFlow(next);
      if (next.browser_open_error) setError(next.browser_open_error);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleCopyLogin = async () => {
    if (!flow) return;
    try {
      await navigator.clipboard.writeText(flow.authorization_url);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      setError(e instanceof Error ? e.message : "复制验证链接失败");
    }
  };

  const handleCancelLogin = async () => {
    if (!flow) return;
    setBusy(true);
    try {
      setFlow(await codexLoginCancel(flow.flow_id));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleLogout = async () => {
    setBusy(true);
    setError(null);
    try {
      await codexLogout();
      setAccount(null);
      await useSettingsStore.getState().load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSetDefault = async () => {
    const { settings, save } = useSettingsStore.getState();
    if (!settings) return;
    const ep = settings.endpoints[CHATGPT_ENDPOINT_KEY];
    await save({
      ...settings,
      default_endpoint: CHATGPT_ENDPOINT_KEY,
      default_model: ep?.active_model ?? CHATGPT_DEFAULT_MODEL,
    });
  };

  const loggedIn = account != null;
  const flowActive = flow?.status === "waiting" || flow?.status === "exchanging";

  return (
    <div className="space-y-2.5 rounded-lg border border-border bg-surface-1 p-3">
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2.5">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent/15 text-accent">
            <Sparkles size={16} />
          </div>
          <div className="min-w-0">
            <div className="flex items-center gap-1.5">
              <p className="text-sm text-gray-200">使用 ChatGPT 登录</p>
              <span
                className="shrink-0 rounded-full border border-amber-500/40 bg-amber-500/10 px-1.5 py-0.5 text-[9px] text-amber-700 dark:text-amber-400"
                title="OpenAI 未提供第三方应用使用 ChatGPT 订阅的官方通道"
              >
                非官方通道
              </span>
            </div>
            {account === undefined ? (
              <p className="text-xs text-gray-600">检查登录状态…</p>
            ) : loggedIn ? (
              <p className="truncate text-xs text-gray-500">
                已登录{account.email ? `：${account.email}` : ""}
                {account.plan ? ` · ${account.plan}` : ""}
              </p>
            ) : (
              <p className="text-xs text-gray-600">
                用 ChatGPT Plus/Pro 订阅，免去手动填 API Key
              </p>
            )}
          </div>
        </div>

        {account === undefined ? null : loggedIn ? (
          <div className="flex shrink-0 items-center gap-2">
            {isDefault ? (
              <span
                className="rounded-full bg-accent/15 px-2 py-0.5 text-[11px] text-accent"
                title="当前模型请求走 ChatGPT 订阅"
              >
                默认
              </span>
            ) : (
              <button
                onClick={handleSetDefault}
                disabled={busy}
                className="rounded border border-border px-2.5 py-1 text-xs text-gray-300 transition-colors hover:bg-surface-3 disabled:opacity-50"
                title="把模型请求切到 ChatGPT 订阅"
              >
                设为默认
              </button>
            )}
            <button
              onClick={handleLogout}
              disabled={busy}
              className="flex items-center gap-1.5 rounded border border-border px-2.5 py-1 text-xs text-gray-400 transition-colors hover:bg-surface-3 disabled:opacity-50"
            >
              <LogOut size={12} /> 退出登录
            </button>
          </div>
        ) : flowActive ? null : (
          <button
            onClick={handleLogin}
            disabled={busy}
            className="flex shrink-0 items-center gap-1.5 rounded bg-accent px-2.5 py-1 text-xs text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
          >
            {busy ? <RefreshCw size={12} className="animate-spin" /> : <LogIn size={12} />}
            {busy ? "正在准备验证…" : flow?.status === "expired" ? "生成新的验证链接" : "登录"}
          </button>
        )}
      </div>

      {flowActive && !loggedIn && (
        <div role="status" aria-live="polite" className="space-y-2 rounded-md border border-border/70 bg-surface-2 p-2.5">
          <div>
            <p className="text-xs font-medium text-gray-200">
              {flow.status === "exchanging" ? "正在完成 ChatGPT 验证" : "在浏览器中完成 ChatGPT 验证"}
            </p>
            <p className="mt-1 text-xs leading-5 text-gray-500">
              若浏览器没有自动打开，可手动打开或复制同一条验证链接。
            </p>
            {flow.browser_open_error && (
              <p className="mt-1 text-xs leading-5 text-amber-700 dark:text-amber-300">
                自动打开失败：{flow.browser_open_error}
              </p>
            )}
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              onClick={() => void handleOpenLogin()}
              disabled={flow.status === "exchanging"}
              className="rounded bg-accent px-2.5 py-1.5 text-xs text-white disabled:opacity-50"
            >
              打开验证页面
            </button>
            <button
              type="button"
              onClick={() => void handleCopyLogin()}
              className="rounded border border-border px-2.5 py-1.5 text-xs text-gray-300 hover:bg-surface-3"
            >
              {copied ? "已复制" : "复制链接"}
            </button>
            <button
              type="button"
              onClick={() => void handleCancelLogin()}
              disabled={busy || flow.status === "exchanging"}
              className="rounded px-2.5 py-1.5 text-xs text-gray-500 hover:bg-surface-3 hover:text-gray-300 disabled:opacity-50"
            >
              取消
            </button>
          </div>
        </div>
      )}
      {error && (
        <p className="flex items-start gap-1.5 text-xs leading-5 text-rose-500">
          <AlertCircle size={12} className="mt-0.5 shrink-0" /> {error}
        </p>
      )}

      {/* Honest labelling: OpenAI documents "Sign in with ChatGPT" for its own
          Codex surfaces (CLI, web, IDE extension, app) and offers no sanctioned
          path for third-party apps to spend a user's subscription. This login
          works, but it is not a supported channel and can stop working without
          notice — say so where the user decides, not in a changelog. */}
      <p className="flex items-start gap-1.5 border-t border-border/70 pt-2 text-[11px] leading-5 text-gray-500">
        <AlertCircle size={11} className="mt-0.5 shrink-0 text-amber-600 dark:text-amber-500" />
        <span>
          这是<span className="text-gray-400">非官方通道</span>：OpenAI 只为自家 Codex
          客户端提供订阅登录，未开放给第三方应用。用量记在你自己的 ChatGPT
          订阅上，通道可能随时失效。要稳定长期使用，请改用 API Key 或其他端点。
        </span>
      </p>
    </div>
  );
}

// ── AboutTab — app version + in-app updater ──────────────────────────────────

// Renders the single status line under "软件更新", driven by the shared
// updater phase. Kept as its own component so the switch stays exhaustive and
// the markup for each phase is easy to read.
function UpdateStatusLine({
  phase,
  currentVersion,
  onInstall,
}: {
  phase: UpdaterPhase;
  currentVersion: string | null;
  onInstall: () => void;
}) {
  switch (phase.kind) {
    case "idle":
      return (
        <p className="text-xs text-gray-500">
          点击「检查更新」以查看是否有新版本。
        </p>
      );
    case "checking":
      return (
        <p className="flex items-center gap-1.5 text-xs text-gray-400">
          <RefreshCw size={12} className="animate-spin" /> 正在检查最新版本…
        </p>
      );
    case "up_to_date":
      return (
        <p className="flex flex-wrap items-center gap-1.5 text-xs text-emerald-700 dark:text-emerald-400">
          <Check size={12} />
          已是最新版本{currentVersion ? ` (v${currentVersion})` : ""}。
          <span className="text-gray-600">
            上次检查 {new Date(phase.checkedAt).toLocaleTimeString()}
          </span>
        </p>
      );
    case "available":
      return (
        <div className="space-y-2">
          <p className="flex items-center gap-1.5 text-xs text-accent">
            <Download size={12} /> 发现新版本{" "}
            <span className="font-semibold">v{phase.update.version}</span>
          </p>
          {phase.update.body && (
            <pre className="max-h-32 overflow-y-auto whitespace-pre-wrap rounded bg-surface-3 p-2 text-[11px] leading-relaxed text-gray-400">
              {phase.update.body}
            </pre>
          )}
          <button
            onClick={onInstall}
            className="flex items-center gap-1.5 rounded bg-accent px-3 py-1.5 text-xs text-white transition-colors hover:bg-accent-hover"
          >
            <Download size={12} /> 下载并安装
          </button>
          <p className="text-[11px] text-gray-600">安装完成后应用会自动重启。</p>
        </div>
      );
    case "downloading": {
      const pct = phase.total ? Math.round((phase.received / phase.total) * 100) : 0;
      return (
        <div className="space-y-1.5">
          <p className="flex items-center gap-1.5 text-xs text-accent">
            <RefreshCw size={12} className="animate-spin" />
            正在下载…{" "}
            {phase.total ? `${pct}%` : `${(phase.received / 1024 / 1024).toFixed(1)} MB`}
          </p>
          <div className="h-1.5 w-full overflow-hidden rounded-full bg-surface-3">
            <div
              className="h-full bg-accent transition-all"
              style={{ width: phase.total ? `${pct}%` : "100%" }}
            />
          </div>
        </div>
      );
    }
    case "installing":
      return (
        <p className="flex items-center gap-1.5 text-xs text-emerald-700 dark:text-emerald-400">
          <RefreshCw size={12} className="animate-spin" /> 正在安装…
        </p>
      );
    case "ready":
      return (
        <p className="flex items-center gap-1.5 text-xs text-emerald-700 dark:text-emerald-400">
          <Check size={12} /> 安装完成，即将重启…
        </p>
      );
    case "error":
      return (
        <p className="flex items-start gap-1.5 text-xs text-rose-500">
          <AlertCircle size={12} className="mt-0.5 shrink-0" />
          检查更新失败：{phase.message}
        </p>
      );
  }
}

const REPO_URL = "https://github.com/BumStill/CodeFactory";

function AboutTab() {
  const phase = useUpdaterStore((s) => s.phase);
  const currentVersion = useUpdaterStore((s) => s.currentVersion);
  const checkNow = useUpdaterStore((s) => s.checkNow);
  const install = useUpdaterStore((s) => s.install);
  const initialize = useUpdaterStore((s) => s.initialize);

  // Ensure the version is resolved even if the floating UpdaterBanner isn't
  // mounted (e.g. Settings opened very early in the session). initialize() is
  // idempotent — it guards its poll handle and re-reading the version is cheap.
  useEffect(() => {
    void initialize();
  }, [initialize]);

  // A check is in flight, or an install is mid-flight — don't let the user
  // kick off a second check on top of either.
  const busy =
    phase.kind === "checking" ||
    phase.kind === "downloading" ||
    phase.kind === "installing" ||
    phase.kind === "ready";

  return (
    <div className="max-w-md space-y-6">
      {/* Identity */}
      <div className="flex items-center gap-3">
        <div className="flex h-12 w-12 items-center justify-center rounded-xl bg-accent/15 text-accent">
          <Package size={22} />
        </div>
        <div>
          <h2 className="text-base font-semibold text-gray-100">CodeFactory</h2>
          <p className="font-mono text-xs text-gray-500">
            {currentVersion ? `v${currentVersion}` : "版本加载中…"}
          </p>
        </div>
      </div>

      {/* Update card */}
      <div className="space-y-3 rounded-lg border border-border bg-surface-1 p-4">
        <div className="flex items-center justify-between">
          <span className="text-xs font-semibold uppercase tracking-wider text-gray-400">
            软件更新
          </span>
          <button
            onClick={() => void checkNow()}
            disabled={busy}
            className="flex items-center gap-1.5 rounded border border-border px-3 py-1 text-xs text-gray-300 transition-colors hover:bg-surface-3 disabled:opacity-50"
          >
            <RefreshCw size={12} className={phase.kind === "checking" ? "animate-spin" : ""} />
            {phase.kind === "checking" ? "检查中…" : "检查更新"}
          </button>
        </div>

        <UpdateStatusLine
          phase={phase}
          currentVersion={currentVersion}
          onInstall={() => void install()}
        />
      </div>

      {/* Project link — opens the repo in the system browser (never in-webview) */}
      <button
        onClick={() => void invoke("plugin:shell|open", { path: REPO_URL }).catch(() => {})}
        className="flex w-full items-center gap-2 rounded-lg border border-border bg-surface-1 px-4 py-2.5 text-xs text-gray-300 transition-colors hover:bg-surface-3"
      >
        <Github size={14} className="text-gray-400" />
        <span className="flex-1 text-left">GitHub 项目主页</span>
        <span className="font-mono text-[10px] text-gray-500">BumStill/CodeFactory</span>
        <ExternalLink size={11} className="text-gray-500" />
      </button>

      {/* Meta */}
      <p className="text-[11px] leading-relaxed text-gray-600">
        CodeFactory 是基于 Tauri 的本地 AI 编码工作台。更新通过 GitHub Releases
        分发；点击「检查更新」会与最新发布版本比对，并在确认后下载安装。
      </p>
    </div>
  );
}

// ── DataSection — export / import / show data dir ────────────────────────────

import { save as saveDialog, open as openDialog } from "@tauri-apps/plugin-dialog";
import { Download as DownloadIcon, Upload as UploadIcon, FolderOpen } from "lucide-react";

function DataSection() {
  const [dataDir, setDataDir] = useState<string>("");
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [msg, setMsg] = useState<{ kind: "ok" | "err"; text: string } | null>(null);

  useEffect(() => {
    invoke<string>("get_data_dir").then(setDataDir).catch(() => {});
  }, []);

  const showMsg = (kind: "ok" | "err", text: string) => {
    setMsg({ kind, text });
    setTimeout(() => setMsg(null), 4000);
  };

  const handleExport = async () => {
    const path = await saveDialog({
      defaultPath: `codefactory-backup-${new Date().toISOString().slice(0, 10)}.cfbkp`,
      filters: [{ name: "CodeFactory 备份", extensions: ["cfbkp"] }],
    });
    if (!path) return;
    setBusy("export");
    try {
      const r = await invoke<{ path: string; size_bytes: number }>("export_user_data", {
        targetPath: path,
      });
      showMsg("ok", `已导出 ${(r.size_bytes / 1024 / 1024).toFixed(2)} MB 到 ${r.path}`);
    } catch (e) {
      showMsg("err", String(e));
    } finally {
      setBusy(null);
    }
  };

  const handleImport = async () => {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "CodeFactory 备份", extensions: ["cfbkp"] }],
    });
    if (!path || typeof path !== "string") return;
    if (
      !confirm(
        "恢复将覆盖当前的设置和会话。旧文件会以 .pre-restore-<timestamp> 后缀保存在数据目录中。是否继续？",
      )
    ) {
      return;
    }
    setBusy("import");
    try {
      const r = await invoke<{ restored_settings: boolean; restored_db: boolean }>(
        "import_user_data",
        { sourcePath: path },
      );
      const parts: string[] = [];
      if (r.restored_settings) parts.push("设置");
      if (r.restored_db) parts.push("会话");
      showMsg(
        "ok",
        `已恢复 ${parts.join(" + ")}。重启应用后生效。`,
      );
    } catch (e) {
      showMsg("err", String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="pt-4 mt-4 border-t border-border space-y-3">
      <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">数据</h2>

      <div className="rounded-lg border border-border bg-surface-1 p-3 space-y-2">
        <div className="text-[11px] text-gray-500">存储位置</div>
        <div className="flex items-center gap-2 font-mono text-[11px] text-gray-300">
          <FolderOpen size={11} className="text-gray-600 shrink-0" />
          <span className="truncate" title={dataDir}>{dataDir || "加载中…"}</span>
        </div>
        <p className="text-[11px] text-gray-600 leading-relaxed">
          所有会话、消息和设置都保存在这里。卸载并重装后依然保留。
          API Key 不包含在设置备份内。macOS 会同时保存系统凭据与权限为 0600 的本机可用性副本；
          删除端点时两份都会清理。
        </p>
      </div>

      <div className="flex gap-2">
        <button
          onClick={handleExport}
          disabled={busy !== null}
          className="flex-1 flex items-center justify-center gap-1.5 px-3 py-2 rounded border border-border bg-surface-1 hover:bg-surface-3 text-xs text-gray-200 transition-colors disabled:opacity-50"
        >
          <DownloadIcon size={12} />
          {busy === "export" ? "导出中…" : "导出备份"}
        </button>
        <button
          onClick={handleImport}
          disabled={busy !== null}
          className="flex-1 flex items-center justify-center gap-1.5 px-3 py-2 rounded border border-border bg-surface-1 hover:bg-surface-3 text-xs text-gray-200 transition-colors disabled:opacity-50"
        >
          <UploadIcon size={12} />
          {busy === "import" ? "恢复中…" : "从备份恢复"}
        </button>
      </div>

      {msg && (
        <div
          className={`text-[11px] rounded border px-2.5 py-1.5 ${
            msg.kind === "ok"
              ? "border-green-500/30 bg-green-500/10 text-green-800 dark:text-green-300"
              : "border-red-500/30 bg-red-500/10 text-red-800 dark:text-red-300"
          }`}
        >
          {msg.text}
        </div>
      )}
    </div>
  );
}
