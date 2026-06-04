// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { ChevronLeft, Plus, Trash2, Tag, Terminal, Download, Store, FolderOpen, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useSkillsStore, type SkillManifest, type SkillDetail } from "../../stores/skills";

interface SkillsPageProps {
  onBack: () => void;
}

type Tab = "installed" | "marketplace";

interface MarketplaceSkill {
  id: string;
  name: string;
  description: string;
  version: string;
  author: string;
  tags: string[];
  system_prompt: string;
  slash_commands: unknown[];
  installed: boolean;
}

const REGISTRY_URL =
  "https://raw.githubusercontent.com/BumStill/codefactory-skills/main/registry.json";

export function SkillsPage({ onBack }: SkillsPageProps) {
  const { skills, loading, loadSkills, enableSkill, disableSkill, installFromUrl, importFromDirectory, createSkill, updateSkill, deleteSkill, getSkillDetail } =
    useSkillsStore();

  const [tab, setTab] = useState<Tab>("installed");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [installUrl, setInstallUrl] = useState("");
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const [togglingId, setTogglingId] = useState<string | null>(null);

  // Create / edit skill form (P3). null = closed.
  const [form, setForm] = useState<{
    mode: "create" | "edit";
    id?: string;
    name: string;
    description: string;
    instructions: string;
  } | null>(null);
  const [savingForm, setSavingForm] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  const handleSaveForm = async () => {
    if (!form || !form.name.trim()) return;
    setSavingForm(true);
    setFormError(null);
    try {
      if (form.mode === "create") {
        await createSkill(form.name.trim(), form.description.trim(), form.instructions);
      } else if (form.id) {
        await updateSkill(form.id, {
          name: form.name.trim(),
          description: form.description.trim(),
          instructions: form.instructions,
        });
        if (selectedId === form.id) {
          setDetail(await getSkillDetail(form.id));
        }
      }
      setForm(null);
    } catch (e) {
      setFormError(String(e));
    } finally {
      setSavingForm(false);
    }
  };

  // Marketplace state
  const [marketSkills, setMarketSkills] = useState<MarketplaceSkill[]>([]);
  const [marketLoading, setMarketLoading] = useState(false);
  const [marketError, setMarketError] = useState<string | null>(null);
  const [usingLocalCatalog, setUsingLocalCatalog] = useState(false);
  const [marketSearch, setMarketSearch] = useState("");
  const [installingId, setInstallingId] = useState<string | null>(null);

  useEffect(() => {
    loadSkills();
  }, []);

  useEffect(() => {
    if (tab === "marketplace" && marketSkills.length === 0 && !marketLoading) {
      loadMarketplace();
    }
  }, [tab]);

  const handleImportDir = async () => {
    setInstallError(null);
    try {
      const dir = await openDialog({
        directory: true,
        title: "选择 skill 目录（含 SKILL.md 或 manifest.json，可整个仓库）",
      });
      if (!dir || typeof dir !== "string") return;
      setInstalling(true);
      await importFromDirectory(dir);
    } catch (e) {
      setInstallError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const loadMarketplace = async () => {
    setMarketLoading(true);
    setMarketError(null);
    try {
      // Try remote first, fallback is handled in Rust
      const skills = await invoke<MarketplaceSkill[]>("fetch_marketplace_skills", {
        registryUrl: REGISTRY_URL,
      });
      setMarketSkills(skills);
      // If all skills come from builtin, indicate local catalog
      setUsingLocalCatalog(false);
    } catch {
      // Last resort: try with null URL to get builtin
      try {
        const skills = await invoke<MarketplaceSkill[]>("fetch_marketplace_skills", {
          registryUrl: null,
        });
        setMarketSkills(skills);
        setUsingLocalCatalog(true);
      } catch (e2) {
        setMarketError(String(e2));
      }
    } finally {
      setMarketLoading(false);
    }
  };

  const handleInstallMarketplace = async (skill: MarketplaceSkill) => {
    setInstallingId(skill.id);
    try {
      await invoke("install_marketplace_skill", { skill });
      await loadSkills();
      // Refresh marketplace to update installed flags
      await loadMarketplace();
    } catch (e) {
      setMarketError(String(e));
    } finally {
      setInstallingId(null);
    }
  };

  const filteredMarketSkills = marketSkills.filter((s) => {
    if (!marketSearch.trim()) return true;
    const q = marketSearch.toLowerCase();
    return (
      s.name.toLowerCase().includes(q) ||
      s.description.toLowerCase().includes(q) ||
      s.tags.some((t) => t.toLowerCase().includes(q))
    );
  });

  const handleSelectSkill = async (id: string) => {
    setSelectedId(id);
    setDetail(null);
    setDetailLoading(true);
    try {
      const d = await getSkillDetail(id);
      setDetail(d);
    } catch (e) {
      console.error(e);
    } finally {
      setDetailLoading(false);
    }
  };

  const handleToggle = async (skill: SkillManifest) => {
    setTogglingId(skill.id);
    try {
      if (skill.enabled) {
        await disableSkill(skill.id);
      } else {
        await enableSkill(skill.id);
      }
      if (selectedId === skill.id) {
        const d = await getSkillDetail(skill.id);
        setDetail(d);
      }
    } finally {
      setTogglingId(null);
    }
  };

  const handleDelete = async (id: string) => {
    await deleteSkill(id);
    if (selectedId === id) {
      setSelectedId(null);
      setDetail(null);
    }
  };

  const handleInstall = async () => {
    if (!installUrl.trim()) return;
    setInstalling(true);
    setInstallError(null);
    try {
      await installFromUrl(installUrl.trim());
      setInstallUrl("");
    } catch (e) {
      setInstallError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  return (
    <div className="flex h-full bg-surface-0">
      {/* Left sidebar */}
      <aside className="w-64 flex-shrink-0 flex flex-col border-r border-border bg-surface-1">
        {/* Header */}
        <div className="flex items-center gap-1 px-3 py-2 border-b border-border">
          <button
            onClick={onBack}
            className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
            title="返回"
          >
            <ChevronLeft size={14} />
          </button>
          <span className="flex-1 text-xs font-semibold text-gray-400 uppercase tracking-wider">
            技能
          </span>
        </div>

        {/* Tab switcher */}
        <div className="flex border-b border-border">
          <button
            onClick={() => setTab("installed")}
            className={`flex-1 py-1.5 text-xs font-medium transition-colors ${
              tab === "installed"
                ? "text-gray-200 border-b-2 border-accent"
                : "text-gray-600 hover:text-gray-400"
            }`}
          >
            已安装
          </button>
          <button
            onClick={() => setTab("marketplace")}
            className={`flex-1 py-1.5 text-xs font-medium flex items-center justify-center gap-1 transition-colors ${
              tab === "marketplace"
                ? "text-gray-200 border-b-2 border-accent"
                : "text-gray-600 hover:text-gray-400"
            }`}
          >
            <Store size={10} />
            市场
          </button>
        </div>

        {tab === "installed" ? (
          <>
            {/* Install from URL */}
            <div className="p-2 border-b border-border space-y-1.5">
              <div className="flex gap-1">
                <input
                  type="text"
                  value={installUrl}
                  onChange={(e) => setInstallUrl(e.target.value)}
                  onKeyDown={(e) => e.key === "Enter" && handleInstall()}
                  placeholder="从 URL 安装…"
                  className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 placeholder-gray-600 outline-none focus:border-accent/50"
                />
                <button
                  onClick={handleInstall}
                  disabled={installing || !installUrl.trim()}
                  className="p-1.5 rounded bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
                  title="从 URL 安装技能"
                >
                  <Plus size={12} />
                </button>
                <button
                  onClick={handleImportDir}
                  disabled={installing}
                  className="p-1.5 rounded bg-surface-3 hover:bg-surface-2 text-gray-300 border border-border disabled:opacity-50 transition-colors"
                  title="从本地目录导入（支持 SKILL.md，如 superpowers / openspec，可整个仓库批量导入）"
                >
                  <FolderOpen size={12} />
                </button>
              </div>
              <button
                onClick={() => {
                  setFormError(null);
                  setForm({ mode: "create", name: "", description: "", instructions: "" });
                }}
                className="w-full flex items-center justify-center gap-1 px-2 py-1 rounded bg-accent/15 hover:bg-accent/25 text-accent text-xs transition-colors"
              >
                <Plus size={11} /> 新建技能
              </button>
              {installError && (
                <p className="text-xs text-red-400 truncate" title={installError}>
                  {installError}
                </p>
              )}
            </div>

            {/* Installed skill list */}
            <ul className="flex-1 overflow-y-auto py-1">
              {loading && (
                <li className="px-3 py-2 text-xs text-gray-700">加载中…</li>
              )}
              {!loading && skills.length === 0 && (
                <li className="px-3 py-2 text-xs text-gray-700">未安装任何技能</li>
              )}
              {skills.map((skill) => (
                <li key={skill.id}>
                  <button
                    className={`group w-full flex flex-col gap-0.5 px-3 py-2 text-left transition-colors ${
                      selectedId === skill.id
                        ? "bg-surface-3 text-gray-200"
                        : "text-gray-500 hover:bg-surface-2 hover:text-gray-300"
                    } ${skill.enabled ? "border-l-2 border-accent" : "border-l-2 border-transparent"}`}
                    onClick={() => handleSelectSkill(skill.id)}
                  >
                    <div className="flex items-center gap-1.5 w-full min-w-0">
                      <span className="flex-1 truncate text-xs font-medium">
                        {skill.name}
                      </span>
                      {skill.source === "user" && (
                        <span
                          className="opacity-0 group-hover:opacity-100 transition-opacity p-0.5 rounded hover:bg-surface-4 text-gray-600 hover:text-red-400"
                          role="button"
                          title="删除"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleDelete(skill.id);
                          }}
                        >
                          <Trash2 size={10} />
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-1 flex-wrap">
                      {skill.enabled && (
                        <span className="px-1 py-0.5 rounded bg-accent/20 text-accent text-[10px]">
                          已启用
                        </span>
                      )}
                      <span className="text-[10px] text-gray-700">
                        {skill.source}
                      </span>
                    </div>
                  </button>
                </li>
              ))}
            </ul>
          </>
        ) : (
          <>
            {/* Marketplace search */}
            <div className="p-2 border-b border-border">
              <input
                type="text"
                value={marketSearch}
                onChange={(e) => setMarketSearch(e.target.value)}
                placeholder="搜索技能…"
                className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-xs text-gray-300 placeholder-gray-600 outline-none focus:border-accent/50"
              />
            </div>

            {/* Marketplace list */}
            <ul className="flex-1 overflow-y-auto py-1">
              {marketLoading && (
                <li className="px-3 py-2 text-xs text-gray-700">正在加载目录…</li>
              )}
              {!marketLoading && marketError && (
                <li className="px-3 py-2 text-xs text-red-400">{marketError}</li>
              )}
              {!marketLoading && !marketError && filteredMarketSkills.length === 0 && (
                <li className="px-3 py-2 text-xs text-gray-700">未找到技能</li>
              )}
              {filteredMarketSkills.map((skill) => (
                <li key={skill.id} className="px-3 py-2 border-b border-border/50">
                  <div className="flex items-start gap-2">
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-1.5">
                        <span className="text-xs font-medium text-gray-300 truncate">
                          {skill.name}
                        </span>
                        {skill.installed && (
                          <span className="px-1 py-0.5 rounded bg-accent/20 text-accent text-[10px] flex-shrink-0">
                            已安装
                          </span>
                        )}
                      </div>
                      <p className="text-[10px] text-gray-600 mt-0.5 line-clamp-2">
                        {skill.description}
                      </p>
                      <div className="flex gap-1 mt-1 flex-wrap">
                        {skill.tags.map((tag) => (
                          <span
                            key={tag}
                            className="flex items-center gap-0.5 px-1 py-0.5 rounded bg-surface-3 text-gray-500 text-[10px]"
                          >
                            <Tag size={7} />
                            {tag}
                          </span>
                        ))}
                      </div>
                    </div>
                    {!skill.installed && (
                      <button
                        onClick={() => handleInstallMarketplace(skill)}
                        disabled={installingId === skill.id}
                        className="flex-shrink-0 p-1 rounded bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
                        title="安装"
                      >
                        {installingId === skill.id ? (
                          <span className="text-[10px]">...</span>
                        ) : (
                          <Download size={10} />
                        )}
                      </button>
                    )}
                  </div>
                </li>
              ))}
            </ul>

            {/* Local catalog notice */}
            {usingLocalCatalog && !marketLoading && (
              <div className="px-3 py-1.5 border-t border-border text-[10px] text-gray-700">
                正在使用本地目录（离线）
              </div>
            )}
          </>
        )}
      </aside>

      {/* Main detail area */}
      <div className="flex flex-1 flex-col min-w-0">
        {tab === "marketplace" ? (
          <MarketplaceWelcome />
        ) : !selectedId ? (
          <div className="flex-1 flex items-center justify-center text-sm text-gray-700">
            选择一个技能查看详情
          </div>
        ) : detailLoading ? (
          <div className="flex-1 flex items-center justify-center text-sm text-gray-700">
            加载中…
          </div>
        ) : detail ? (
          <SkillDetailView
            detail={detail}
            toggling={togglingId === detail.manifest.id}
            onToggle={() => handleToggle(detail.manifest)}
            onEdit={() => {
              setFormError(null);
              setForm({
                mode: "edit",
                id: detail.manifest.id,
                name: detail.manifest.name,
                description: detail.manifest.description,
                instructions: detail.system_prompt,
              });
            }}
          />
        ) : null}
      </div>

      {form && (
        <SkillFormModal
          form={form}
          saving={savingForm}
          error={formError}
          onChange={setForm}
          onCancel={() => setForm(null)}
          onSave={handleSaveForm}
        />
      )}
    </div>
  );
}

function SkillFormModal({
  form,
  saving,
  error,
  onChange,
  onCancel,
  onSave,
}: {
  form: { mode: "create" | "edit"; id?: string; name: string; description: string; instructions: string };
  saving: boolean;
  error: string | null;
  onChange: (f: { mode: "create" | "edit"; id?: string; name: string; description: string; instructions: string }) => void;
  onCancel: () => void;
  onSave: () => void;
}) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4" onClick={onCancel}>
      <div
        className="w-full max-w-lg rounded-lg border border-border bg-surface-1 shadow-2xl flex flex-col max-h-[85vh]"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex items-center justify-between px-4 py-3 border-b border-border">
          <h2 className="text-sm font-semibold text-gray-200">
            {form.mode === "create" ? "新建技能" : "编辑技能"}
          </h2>
          <button onClick={onCancel} className="p-1 rounded text-gray-500 hover:text-gray-200 hover:bg-surface-3">
            <X size={14} />
          </button>
        </header>
        <div className="flex-1 overflow-y-auto p-4 space-y-3">
          <div>
            <label className="block text-[11px] text-gray-500 mb-1">名称</label>
            <input
              type="text"
              autoFocus
              value={form.name}
              onChange={(e) => onChange({ ...form, name: e.target.value })}
              placeholder="例如 周报助手"
              className="w-full bg-surface-2 border border-border rounded px-2 py-1.5 text-sm text-gray-200 outline-none focus:border-accent"
            />
          </div>
          <div>
            <label className="block text-[11px] text-gray-500 mb-1">描述（何时使用）</label>
            <input
              type="text"
              value={form.description}
              onChange={(e) => onChange({ ...form, description: e.target.value })}
              className="w-full bg-surface-2 border border-border rounded px-2 py-1.5 text-sm text-gray-200 outline-none focus:border-accent"
            />
          </div>
          <div>
            <label className="block text-[11px] text-gray-500 mb-1">技能指令（启用后注入系统提示）</label>
            <textarea
              value={form.instructions}
              onChange={(e) => onChange({ ...form, instructions: e.target.value })}
              rows={10}
              placeholder="用自然语言写下这个技能要让 AI 怎么做…"
              className="w-full bg-surface-2 border border-border rounded px-2 py-1.5 text-[13px] text-gray-200 outline-none focus:border-accent resize-y font-mono leading-relaxed"
            />
          </div>
          {error && <p className="text-xs text-red-400 break-words">{error}</p>}
        </div>
        <footer className="flex items-center justify-end gap-2 px-4 py-3 border-t border-border">
          <button onClick={onCancel} className="px-3 py-1.5 rounded text-xs text-gray-400 hover:bg-surface-3">
            取消
          </button>
          <button
            onClick={onSave}
            disabled={saving || !form.name.trim()}
            className="px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-xs disabled:opacity-40"
          >
            {saving ? "保存中…" : form.mode === "create" ? "创建" : "保存"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function MarketplaceWelcome() {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-3 text-center px-8">
      <Store size={32} className="text-gray-700" />
      <div>
        <h2 className="text-sm font-medium text-gray-400">技能市场</h2>
        <p className="text-xs text-gray-700 mt-1 max-w-xs">
          浏览并安装社区技能。点击“安装”将技能添加到你的技能库，然后在“已安装”标签页中启用它。
        </p>
      </div>
    </div>
  );
}

function SkillDetailView({
  detail,
  toggling,
  onToggle,
  onEdit,
}: {
  detail: SkillDetail;
  toggling: boolean;
  onToggle: () => void;
  onEdit: () => void;
}) {
  const { manifest, system_prompt, slash_commands, has_tool_policy } = detail;

  return (
    <div className="flex flex-col h-full overflow-y-auto p-5 gap-5">
      {/* Header */}
      <div className="flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <h1 className="text-sm font-semibold text-gray-200">{manifest.name}</h1>
          <p className="text-xs text-gray-500 mt-0.5">{manifest.description}</p>
          <div className="flex items-center gap-2 mt-1 flex-wrap">
            <span className="text-[10px] text-gray-700">v{manifest.version} 作者 {manifest.author}</span>
            {manifest.tags.map((tag) => (
              <span
                key={tag}
                className="flex items-center gap-0.5 px-1.5 py-0.5 rounded bg-surface-3 text-gray-400 text-[10px]"
              >
                <Tag size={8} />
                {tag}
              </span>
            ))}
            {has_tool_policy && (
              <span className="px-1.5 py-0.5 rounded bg-yellow-900/40 text-yellow-400 text-[10px]">
                工具策略
              </span>
            )}
          </div>
        </div>
        {manifest.source === "user" && (
          <button
            onClick={onEdit}
            className="px-3 py-1.5 rounded text-xs font-medium bg-surface-3 text-gray-400 hover:text-gray-200 hover:bg-surface-4 transition-colors"
          >
            编辑
          </button>
        )}
        <button
          onClick={onToggle}
          disabled={toggling}
          className={`px-3 py-1.5 rounded text-xs font-medium transition-colors disabled:opacity-50 ${
            manifest.enabled
              ? "bg-surface-3 text-gray-400 hover:text-red-400 hover:bg-surface-4"
              : "bg-accent hover:bg-accent-hover text-white"
          }`}
        >
          {toggling ? "…" : manifest.enabled ? "禁用" : "启用"}
        </button>
      </div>

      {/* System prompt */}
      <div>
        <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2">
          系统提示词
        </h2>
        <pre className="text-xs text-gray-300 bg-surface-1 border border-border rounded p-3 whitespace-pre-wrap leading-relaxed font-mono">
          {system_prompt || "（无）"}
        </pre>
      </div>

      {/* Slash commands */}
      {slash_commands.length > 0 && (
        <div>
          <h2 className="text-xs font-semibold text-gray-500 uppercase tracking-wider mb-2">
            斜杠命令
          </h2>
          <div className="space-y-1.5">
            {slash_commands.map((cmd) => (
              <div
                key={cmd.name}
                className="flex items-start gap-3 rounded border border-border bg-surface-1 px-3 py-2"
              >
                <div className="flex items-center gap-1 flex-shrink-0">
                  <Terminal size={10} className="text-accent" />
                  <code className="text-xs text-accent font-mono">/{cmd.name}</code>
                </div>
                <div className="flex-1 min-w-0">
                  <div className="text-xs text-gray-300">{cmd.description}</div>
                  <div className="text-[10px] text-gray-600 mt-0.5 font-mono truncate">
                    {cmd.template}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
