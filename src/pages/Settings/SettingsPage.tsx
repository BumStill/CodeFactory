// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft, Plus, Trash2, Eye, EyeOff, Check, AlertCircle, ChevronDown,
} from "lucide-react";
import { invoke } from "../../lib/tauri";
import { useSettingsStore } from "../../stores/settings";
import { useChatStore } from "../../stores/chat";
import { useGitRemoteStore } from "../../stores/gitRemote";
import type { Settings, Endpoint, ApiStyle, CustomModel, GitRemoteConfig, GitProvider } from "../../lib/tauri";

interface Props {
  onBack: () => void;
}

type Tab = "endpoints" | "permissions" | "general" | "hooks" | "remotes";

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

function TagList({
  label,
  tags,
  onChange,
}: {
  label: string;
  tags: string[];
  onChange: (t: string[]) => void;
}) {
  const [input, setInput] = useState("");

  const add = () => {
    const v = input.trim();
    if (v && !tags.includes(v)) onChange([...tags, v]);
    setInput("");
  };

  return (
    <div className="space-y-1.5">
      <span className="text-xs text-gray-500">{label}</span>
      <div className="flex flex-wrap gap-1 min-h-[28px]">
        {tags.map((t) => (
          <span
            key={t}
            className="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-surface-3 text-xs text-gray-300"
          >
            {t}
            <button
              onClick={() => onChange(tags.filter((x) => x !== t))}
              className="text-gray-600 hover:text-red-400"
            >
              ×
            </button>
          </span>
        ))}
      </div>
      <div className="flex gap-1">
        <input
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && add()}
          placeholder="Add tool name…"
          className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
        />
        <button
          onClick={add}
          className="px-2 py-1 rounded bg-surface-3 hover:bg-surface-4 text-xs text-gray-400 border border-border"
        >
          Add
        </button>
      </div>
    </div>
  );
}

// ── Endpoint editor ───────────────────────────────────────────────────────────

interface EndpointDraft {
  key: string;       // map key in settings.endpoints
  base_url: string;
  api_style: ApiStyle;
  api_key: string;   // loaded/saved separately via keychain
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
          Custom Models
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
              No custom models yet. Useful for LMStudio / Ollama / private gateways
              that don't expose <code>/models</code>.
            </div>
          )}

          {models.map((m, idx) => (
            <div key={idx} className="flex items-center gap-1.5">
              <input
                value={m.id}
                onChange={(e) => updateAt(idx, { id: e.target.value })}
                placeholder="model-id (e.g. llama3.1:8b)"
                className="flex-[2] bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
              />
              <input
                value={m.name ?? ""}
                onChange={(e) =>
                  updateAt(idx, { name: e.target.value || undefined })
                }
                placeholder="display name (optional)"
                className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
              />
              <button
                onClick={() => removeAt(idx)}
                className="p-1 text-gray-600 hover:text-red-400 transition-colors"
                title="Remove"
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
            <Plus size={11} /> Add model
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
              default
            </span>
          ) : (
            <button
              onClick={onSetDefault}
              className="text-[10px] px-1.5 py-0.5 rounded border border-border text-gray-500 hover:text-gray-300 hover:border-gray-500 transition-colors"
            >
              set default
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
        <label className="text-[11px] text-gray-500">Base URL</label>
        <input
          value={local.base_url}
          onChange={(e) => setLocal({ ...local, base_url: e.target.value })}
          placeholder="https://openrouter.ai/api/v1"
          className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
        />
      </div>

      {/* API Key */}
      <div className="space-y-1">
        <label className="text-[11px] text-gray-500">API Key</label>
        <div className="flex gap-1">
          <input
            type={showKey ? "text" : "password"}
            value={local.api_key}
            onChange={(e) => setLocal({ ...local, api_key: e.target.value })}
            placeholder="sk-…"
            className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
          />
          <button
            onClick={() => setShowKey((v) => !v)}
            className="p-1 rounded border border-border text-gray-500 hover:text-gray-300"
          >
            {showKey ? <EyeOff size={12} /> : <Eye size={12} />}
          </button>
        </div>
      </div>

      {/* API Style */}
      <div className="space-y-1">
        <label className="text-[11px] text-gray-500">API Style</label>
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
                {style === "openai" ? "OpenAI-compatible" : "Anthropic Messages"}
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
            {saved ? <><Check size={11} /> Saved</> : saving ? "Saving…" : "Save"}
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
    if (!k) { setErr("Name required"); return; }
    if (existing.includes(k)) { setErr(`"${k}" already exists`); return; }
    if (!url.trim()) { setErr("Base URL required"); return; }
    onAdd(k, { base_url: url.trim(), api_style: style, custom_models: [] });
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        className="w-96 rounded-xl border border-border bg-surface-2 shadow-2xl p-4 space-y-3"
        onClick={(e) => e.stopPropagation()}
      >
        <h3 className="text-sm font-semibold text-gray-200">Add Endpoint</h3>

        <div className="space-y-1">
          <label className="text-[11px] text-gray-500">Name (slug)</label>
          <input
            autoFocus
            value={key}
            onChange={(e) => { setKey(e.target.value); setErr(""); }}
            placeholder="my-endpoint"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-gray-500">Base URL</label>
          <input
            value={url}
            onChange={(e) => { setUrl(e.target.value); setErr(""); }}
            placeholder="https://api.example.com/v1"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] text-gray-500">API Style</label>
          <div className="flex gap-3">
            {(["openai", "anthropic"] as ApiStyle[]).map((s) => (
              <label key={s} className="flex items-center gap-1.5 cursor-pointer">
                <input type="radio" checked={style === s} onChange={() => setStyle(s)} />
                <span className="text-xs text-gray-300">
                  {s === "openai" ? "OpenAI-compatible" : "Anthropic Messages"}
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
            Cancel
          </button>
          <button onClick={handleAdd} className="px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-xs text-white transition-colors">
            Add
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Inline model selector for General tab ─────────────────────────────────────

interface ModelSelectorProps {
  value: string;
  endpointName: string;
  onChange: (id: string) => void;
}

function ModelSelector({ value, endpointName, onChange }: ModelSelectorProps) {
  const [models, setModels] = useState<Array<{ id: string; name: string }>>([]);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  // Load model list when dropdown opens (or endpoint changes)
  useEffect(() => {
    setLoading(true);
    invoke<Array<{ id: string; name: string }>>("list_models", { endpointName })
      .then(setModels)
      .catch(() => setModels([]))
      .finally(() => setLoading(false));
  }, [endpointName]);

  useEffect(() => {
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, []);

  const displayed = value.split("/").pop() || value || "Select model…";
  const filtered = models.filter(
    (m) => !query || m.id.toLowerCase().includes(query.toLowerCase()) || m.name.toLowerCase().includes(query.toLowerCase())
  );

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center justify-between gap-2 bg-surface-3 border border-border rounded px-3 py-1.5 text-xs text-gray-200 hover:border-accent/40 transition-colors"
      >
        <span className="truncate text-left">{displayed}</span>
        <ChevronDown size={12} className="flex-shrink-0 text-gray-500" />
      </button>
      {open && (
        <div className="absolute left-0 top-full mt-1 z-50 w-full rounded-lg border border-border bg-surface-2 shadow-xl">
          <div className="p-2 border-b border-border">
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search models…"
              className="w-full bg-surface-3 rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none"
            />
          </div>
          <ul className="max-h-56 overflow-y-auto py-1">
            {loading && <li className="px-3 py-2 text-xs text-gray-600">Loading…</li>}
            {!loading && filtered.length === 0 && (
              <li className="px-3 py-2 text-xs text-gray-600">No models found</li>
            )}
            {filtered.slice(0, 60).map((m) => (
              <li key={m.id}>
                <button
                  className={`w-full text-left px-3 py-1.5 text-xs hover:bg-surface-3 transition-colors truncate ${
                    m.id === value ? "text-accent" : "text-gray-300"
                  }`}
                  onClick={() => { onChange(m.id); setQuery(""); setOpen(false); }}
                >
                  {m.id}
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

// ── Main Settings Page ────────────────────────────────────────────────────────

export function SettingsPage({ onBack }: Props) {
  const { settings, load, save, saveApiKey, getApiKey } = useSettingsStore();
  const [tab, setTab] = useState<Tab>("endpoints");
  const [endpointDrafts, setEndpointDrafts] = useState<EndpointDraft[]>([]);
  const [showAddEp, setShowAddEp] = useState(false);
  const [permDraft, setPermDraft] = useState<Settings["permissions"] | null>(null);
  const [permSaved, setPermSaved] = useState(false);
  const [generalDraft, setGeneralDraft] = useState<{
    default_model: string;
    shell: string;
    auto_create_pr: boolean;
  } | null>(null);
  const [generalSaved, setGeneralSaved] = useState(false);

  // ── Load ───────────────────────────────────────────────────────────────────

  useEffect(() => {
    load();
  }, []);

  useEffect(() => {
    if (!settings) return;

    // Build endpoint drafts — load API keys in parallel
    const keys = Object.keys(settings.endpoints);
    Promise.all(
      keys.map(async (k) => {
        const ep = settings.endpoints[k];
        const apiKey = ep.key_ref ? (await getApiKey(ep.key_ref)) ?? "" : "";
        const draft: EndpointDraft = {
          key: k,
          base_url: ep.base_url,
          api_style: ep.api_style,
          api_key: apiKey,
          key_ref: ep.key_ref,
          custom_models: ep.custom_models ?? [],
        };
        return draft;
      })
    ).then((drafts) => setEndpointDrafts(drafts));

    setPermDraft({ ...settings.permissions });
    setGeneralDraft({
      default_model: settings.default_model,
      shell: settings.shell.shell,
      auto_create_pr: (settings as Settings & { auto_create_pr?: boolean }).auto_create_pr ?? false,
    });
  }, [settings]);

  if (!settings || !permDraft || !generalDraft) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-gray-600">
        Loading settings…
      </div>
    );
  }

  // ── Endpoint handlers ──────────────────────────────────────────────────────

  const handleSaveEndpoint = async (draft: EndpointDraft) => {
    // Persist API key to keychain
    const keyRef = draft.key_ref ?? `codefactory.endpoint.${draft.key}`;
    if (draft.api_key) {
      await saveApiKey(keyRef, draft.api_key);
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
      key_ref: keyRef,
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
          ? { ...draft, key_ref: keyRef, custom_models: cleanedModels }
          : d,
      ),
    );
  };

  const handleSetDefault = async (key: string) => {
    await save({ ...settings, default_endpoint: key });
  };

  const handleDeleteEndpoint = async (key: string) => {
    if (settings.default_endpoint === key) return; // can't delete default
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
        custom_models: ep.custom_models ?? [],
      },
    ]);
  };

  // ── Permissions handlers ───────────────────────────────────────────────────

  const handleSavePerms = async () => {
    if (!permDraft) return;
    await save({ ...settings, permissions: permDraft });
    setPermSaved(true);
    setTimeout(() => setPermSaved(false), 1500);
  };

  // ── General handlers ───────────────────────────────────────────────────────

  const handleSaveGeneral = async () => {
    if (!generalDraft) return;
    await save({
      ...settings,
      default_model: generalDraft.default_model,
      shell: { shell: generalDraft.shell },
      auto_create_pr: generalDraft.auto_create_pr,
    } as Settings & { auto_create_pr: boolean });

    // Sync to chat store: if no active session, update the displayed model immediately;
    // if there is an active session, only update if user is still on the default
    // (i.e. the session model matches the old default, meaning they haven't customised it).
    const chatState = useChatStore.getState();
    if (!chatState.activeSession) {
      chatState.setModel(generalDraft.default_model);
    } else if (chatState.activeModel === settings.default_model) {
      // Session was still using the global default — follow the change
      chatState.setModel(generalDraft.default_model);
    }

    setGeneralSaved(true);
    setTimeout(() => setGeneralSaved(false), 1500);
  };

  // ── Render ─────────────────────────────────────────────────────────────────

  const tabs: { id: Tab; label: string }[] = [
    { id: "endpoints", label: "Endpoints" },
    { id: "permissions", label: "Permissions" },
    { id: "general", label: "General" },
    { id: "hooks", label: "Hooks" },
    { id: "remotes", label: "Remotes" },
  ];

  return (
    <div className="flex h-full flex-col bg-surface-0 text-gray-200">
      {/* Header */}
      <header className="flex items-center gap-3 px-4 py-2.5 border-b border-border bg-surface-1 shrink-0">
        <button
          onClick={onBack}
          className="p-1 rounded text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors"
        >
          <ArrowLeft size={14} />
        </button>
        <span className="text-sm font-semibold">Settings</span>

        {/* Tabs */}
        <div className="ml-4 flex gap-1">
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

        {/* ── Endpoints ── */}
        {tab === "endpoints" && (
          <div className="max-w-xl space-y-3">
            <div className="flex items-center justify-between mb-1">
              <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
                API Endpoints
              </h2>
              <button
                onClick={() => setShowAddEp(true)}
                className="flex items-center gap-1 px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 transition-colors border border-border"
              >
                <Plus size={11} /> Add endpoint
              </button>
            </div>

            {endpointDrafts.map((d) => (
              <EndpointCard
                key={d.key}
                draft={d}
                isDefault={settings.default_endpoint === d.key}
                onSetDefault={() => handleSetDefault(d.key)}
                onSave={handleSaveEndpoint}
                onDelete={() => handleDeleteEndpoint(d.key)}
              />
            ))}

            {endpointDrafts.length === 0 && (
              <p className="text-xs text-gray-600">No endpoints configured.</p>
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

        {/* ── Permissions ── */}
        {tab === "permissions" && (
          <div className="max-w-xl space-y-4">
            <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
              Tool Permissions
            </h2>

            <label className="flex items-start gap-3 rounded-lg border border-border bg-surface-1 px-3 py-2.5 cursor-pointer">
              <input
                type="checkbox"
                checked={permDraft.full_access}
                onChange={(e) =>
                  setPermDraft({ ...permDraft, full_access: e.target.checked })
                }
                className="mt-0.5"
              />
              <span>
                <span className="block text-xs font-medium text-gray-200">Full access mode</span>
                <span className="block text-xs leading-5 text-gray-500">
                  Skip all permission prompts. Use only in fully trusted projects.
                </span>
              </span>
            </label>

            <div className="space-y-3 rounded-lg border border-border bg-surface-1 p-3">
              <TagList
                label="Allow (auto-approve)"
                tags={permDraft.allow}
                onChange={(t) => setPermDraft({ ...permDraft, allow: t })}
              />
              <TagList
                label="Ask (prompt user)"
                tags={permDraft.ask}
                onChange={(t) => setPermDraft({ ...permDraft, ask: t })}
              />
              <TagList
                label="Deny (always block)"
                tags={permDraft.deny}
                onChange={(t) => setPermDraft({ ...permDraft, deny: t })}
              />
            </div>

            <div className="flex justify-end">
              <button
                onClick={handleSavePerms}
                className="flex items-center gap-1 px-4 py-1.5 rounded bg-accent hover:bg-accent-hover text-xs text-white transition-colors"
              >
                {permSaved ? <><Check size={11} /> Saved</> : "Save permissions"}
              </button>
            </div>
          </div>
        )}

        {/* ── General ── */}
        {tab === "general" && (
          <div className="max-w-xl space-y-4">
            <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">
              General
            </h2>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">Default Model</label>
              <ModelSelector
                value={generalDraft.default_model}
                endpointName={settings.default_endpoint}
                onChange={(id) => setGeneralDraft({ ...generalDraft, default_model: id })}
              />
              <p className="text-[11px] text-gray-600">
                Used when creating a new session. Existing sessions keep their own model.
              </p>
            </div>

            <div className="space-y-1">
              <label className="text-xs text-gray-500">Shell</label>
              <div className="flex gap-3">
                {["powershell", "cmd", "bash"].map((s) => (
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
                <span className="block text-xs font-medium text-gray-200">Auto-create PR after implementation</span>
                <span className="block text-xs leading-5 text-gray-500">
                  Automatically opens a pull request when a spec implementation finishes successfully.
                </span>
              </span>
            </label>

            <div className="flex justify-end">
              <button
                onClick={handleSaveGeneral}
                className="flex items-center gap-1 px-4 py-1.5 rounded bg-accent hover:bg-accent-hover text-xs text-white transition-colors"
              >
                {generalSaved ? <><Check size={11} /> Saved</> : "Save"}
              </button>
            </div>

            <DataSection />
          </div>
        )}

        {/* ── Hooks ── */}
        {tab === "hooks" && <HooksTab />}

        {/* ── Remotes ── */}
        {tab === "remotes" && <RemotesTab />}

      </div>
    </div>
  );
}

// ── HooksTab ──────────────────────────────────────────────────────────────────

const HOOK_EVENTS = [
  "pre_tool", "post_tool", "pre_task", "post_task",
  "session_start", "session_end", "spec_approved", "verification_failed",
];

const HOOK_ACTIONS: { value: HookActionType; label: string; placeholder: string }[] = [
  { value: "log_to_file",     label: "Log to file",             placeholder: "C:\\logs\\codefactory.jsonl" },
  { value: "run_command",     label: "Run command",             placeholder: "echo hook fired" },
  { value: "emit_event",      label: "Emit Tauri event",        placeholder: "my-hook-event" },
  { value: "auto_git_commit", label: "Auto git commit (post_task)", placeholder: "chore: {task_title}" },
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
        <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">Hooks</h2>
        <button
          onClick={() => setAddOpen(true)}
          className="flex items-center gap-1 px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 border border-border transition-colors"
        >
          <Plus size={11} /> Add hook
        </button>
      </div>

      {hooks.length === 0 && <p className="text-xs text-gray-600">No hooks configured.</p>}

      {hooks.map((hook) => (
        <div key={hook.id} className="rounded-lg border border-border bg-surface-1 px-3 py-2 space-y-1.5">
          <div className="flex items-center gap-2">
            <span className="flex-1 text-xs font-medium text-gray-200 truncate">{hook.name}</span>
            <span className="text-[10px] bg-surface-3 text-gray-500 px-1.5 py-0.5 rounded">{hook.event}</span>
            <button
              onClick={() => handleToggle(hook)}
              className={`text-[10px] px-1.5 py-0.5 rounded transition-colors ${
                hook.enabled ? "bg-accent/20 text-accent" : "bg-surface-3 text-gray-600"
              }`}
            >
              {hook.enabled ? "on" : "off"}
            </button>
            <button
              onClick={() => handleTest(hook.id)}
              className="text-[10px] text-gray-600 hover:text-gray-300 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors"
            >
              test
            </button>
            <button
              onClick={() => handleDelete(hook.id)}
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
    if (!name.trim() || !actionParam.trim()) { setErr("Name and action param required."); return; }
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
      <p className="text-xs font-medium text-gray-300">New Hook</p>
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">Name</label>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="My Hook"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">Event</label>
          <select value={event} onChange={(e) => setEvent(e.target.value)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none">
            {HOOK_EVENTS.map((ev) => <option key={ev} value={ev}>{ev}</option>)}
          </select>
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">Action type</label>
          <select value={actionType} onChange={(e) => setActionType(e.target.value as HookActionType)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none">
            {HOOK_ACTIONS.map((a) => <option key={a.value} value={a.value}>{a.label}</option>)}
          </select>
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">Filter (optional)</label>
          <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="e.g. bash"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">{currentAction?.label ?? "Param"}</label>
          <input value={actionParam} onChange={(e) => setActionParam(e.target.value)}
            placeholder={currentAction?.placeholder ?? ""}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
      </div>
      {err && <p className="text-xs text-red-400">{err}</p>}
      <div className="flex justify-end gap-2">
        <button onClick={onCancel} className="px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300">Cancel</button>
        <button onClick={handleSave} disabled={saving}
          className="px-2 py-1 rounded bg-accent hover:bg-accent-hover text-xs text-white disabled:opacity-50 transition-colors">
          {saving ? "Adding…" : "Add Hook"}
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

  useEffect(() => { loadRemotes(); }, [loadRemotes]);

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
          Git Remotes (GitHub / GitLab)
        </h2>
        <button
          onClick={() => setAddOpen(true)}
          className="flex items-center gap-1 px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300 hover:bg-surface-3 border border-border transition-colors"
        >
          <Plus size={11} /> Add remote
        </button>
      </div>

      {remotes.length === 0 && <p className="text-xs text-gray-600">No remotes configured.</p>}

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
              className="text-[10px] text-gray-600 hover:text-gray-300 px-1.5 py-0.5 rounded hover:bg-surface-3 transition-colors disabled:opacity-50"
            >
              {testing === remote.id ? "…" : "Test"}
            </button>
            <button
              onClick={() => deleteRemote(remote.id)}
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
  addRemote: (config: GitRemoteConfig) => Promise<void>;
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
    if (!name.trim() || !token.trim()) { setErr("Name and token required."); return; }
    setSaving(true); setErr(null);
    try {
      await addRemote({
        id: "", name: name.trim(), provider,
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
      <p className="text-xs font-medium text-gray-300">New Remote</p>
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">Name</label>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="My GitHub"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
        <div>
          <label className="block text-[10px] text-gray-500 mb-0.5">Provider</label>
          <select value={provider} onChange={(e) => handleProviderChange(e.target.value as GitProvider)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none">
            <option value="github">GitHub</option>
            <option value="gitlab">GitLab</option>
          </select>
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">Base URL</label>
          <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)}
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 outline-none focus:border-accent/40" />
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">Personal Access Token</label>
          <div className="flex gap-1">
            <input type={showToken ? "text" : "password"} value={token} onChange={(e) => setToken(e.target.value)}
              placeholder="ghp_…"
              className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
            <button onClick={() => setShowToken((v) => !v)}
              className="p-1 rounded border border-border text-gray-500 hover:text-gray-300">
              {showToken ? <EyeOff size={12} /> : <Eye size={12} />}
            </button>
          </div>
        </div>
        <div className="col-span-2">
          <label className="block text-[10px] text-gray-500 mb-0.5">Default Repo (optional)</label>
          <input value={defaultRepo} onChange={(e) => setDefaultRepo(e.target.value)} placeholder="owner/repo"
            className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40" />
        </div>
      </div>
      {err && <p className="text-xs text-red-400">{err}</p>}
      <div className="flex justify-end gap-2">
        <button onClick={onCancel} className="px-2 py-1 rounded text-xs text-gray-500 hover:text-gray-300">Cancel</button>
        <button onClick={handleSave} disabled={saving}
          className="px-2 py-1 rounded bg-accent hover:bg-accent-hover text-xs text-white disabled:opacity-50 transition-colors">
          {saving ? "Adding…" : "Add Remote"}
        </button>
      </div>
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
      filters: [{ name: "CodeFactory backup", extensions: ["cfbkp"] }],
    });
    if (!path) return;
    setBusy("export");
    try {
      const r = await invoke<{ path: string; size_bytes: number }>("export_user_data", {
        targetPath: path,
      });
      showMsg("ok", `Exported ${(r.size_bytes / 1024 / 1024).toFixed(2)} MB to ${r.path}`);
    } catch (e) {
      showMsg("err", String(e));
    } finally {
      setBusy(null);
    }
  };

  const handleImport = async () => {
    const path = await openDialog({
      multiple: false,
      filters: [{ name: "CodeFactory backup", extensions: ["cfbkp"] }],
    });
    if (!path || typeof path !== "string") return;
    if (
      !confirm(
        "Restoring will overwrite your current settings and sessions. The old files will be saved with a .pre-restore-<timestamp> suffix in the data directory. Continue?",
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
      if (r.restored_settings) parts.push("settings");
      if (r.restored_db) parts.push("sessions");
      showMsg(
        "ok",
        `Restored ${parts.join(" + ")}. Restart the app to take effect.`,
      );
    } catch (e) {
      showMsg("err", String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="pt-4 mt-4 border-t border-border space-y-3">
      <h2 className="text-xs font-semibold text-gray-400 uppercase tracking-wider">Data</h2>

      <div className="rounded-lg border border-border bg-surface-1 p-3 space-y-2">
        <div className="text-[11px] text-gray-500">Storage location</div>
        <div className="flex items-center gap-2 font-mono text-[11px] text-gray-300">
          <FolderOpen size={11} className="text-gray-600 shrink-0" />
          <span className="truncate" title={dataDir}>{dataDir || "loading…"}</span>
        </div>
        <p className="text-[11px] text-gray-600 leading-relaxed">
          All sessions, messages, and settings live here. Survives uninstall and reinstall.
          API keys are stored separately in Windows Credential Manager and are not part of backups.
        </p>
      </div>

      <div className="flex gap-2">
        <button
          onClick={handleExport}
          disabled={busy !== null}
          className="flex-1 flex items-center justify-center gap-1.5 px-3 py-2 rounded border border-border bg-surface-1 hover:bg-surface-3 text-xs text-gray-200 transition-colors disabled:opacity-50"
        >
          <DownloadIcon size={12} />
          {busy === "export" ? "Exporting…" : "Export backup"}
        </button>
        <button
          onClick={handleImport}
          disabled={busy !== null}
          className="flex-1 flex items-center justify-center gap-1.5 px-3 py-2 rounded border border-border bg-surface-1 hover:bg-surface-3 text-xs text-gray-200 transition-colors disabled:opacity-50"
        >
          <UploadIcon size={12} />
          {busy === "import" ? "Restoring…" : "Restore from backup"}
        </button>
      </div>

      {msg && (
        <div
          className={`text-[11px] rounded border px-2.5 py-1.5 ${
            msg.kind === "ok"
              ? "border-green-500/30 bg-green-500/10 text-green-300"
              : "border-red-500/30 bg-red-500/10 text-red-300"
          }`}
        >
          {msg.text}
        </div>
      )}
    </div>
  );
}
