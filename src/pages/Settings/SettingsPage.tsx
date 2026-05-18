// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import {
  ArrowLeft, Plus, Trash2, Eye, EyeOff, Check, AlertCircle,
} from "lucide-react";
import { useSettingsStore } from "../../stores/settings";
import type { Settings, Endpoint, ApiStyle } from "../../lib/tauri";

interface Props {
  onBack: () => void;
}

type Tab = "endpoints" | "permissions" | "general";

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
    local.api_key !== draft.api_key;

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
    onAdd(k, { base_url: url.trim(), api_style: style });
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

    // Rebuild endpoints map
    const newEndpoints: Record<string, Endpoint> = { ...settings.endpoints };
    newEndpoints[draft.key] = {
      base_url: draft.base_url,
      api_style: draft.api_style,
      key_ref: keyRef,
    };
    await save({ ...settings, endpoints: newEndpoints });

    // Update local drafts state
    setEndpointDrafts((prev) =>
      prev.map((d) => (d.key === draft.key ? { ...draft, key_ref: keyRef } : d))
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
      { key, base_url: ep.base_url, api_style: ep.api_style, api_key: "" },
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
    setGeneralSaved(true);
    setTimeout(() => setGeneralSaved(false), 1500);
  };

  // ── Render ─────────────────────────────────────────────────────────────────

  const tabs: { id: Tab; label: string }[] = [
    { id: "endpoints", label: "Endpoints" },
    { id: "permissions", label: "Permissions" },
    { id: "general", label: "General" },
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
              <label className="text-xs text-gray-500">Default Model ID</label>
              <input
                value={generalDraft.default_model}
                onChange={(e) =>
                  setGeneralDraft({ ...generalDraft, default_model: e.target.value })
                }
                placeholder="anthropic/claude-opus-4-7"
                className="w-full bg-surface-3 border border-border rounded px-3 py-1.5 text-xs text-gray-200 placeholder-gray-600 outline-none focus:border-accent/40"
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
          </div>
        )}
      </div>
    </div>
  );
}
