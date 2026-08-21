// SPDX-License-Identifier: Apache-2.0
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { ChevronLeft, Plus, Trash2, Tag, Terminal, Download, Store, FolderOpen, X, Sparkles, Loader2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import {
  useSkillsStore,
  type SkillManifest,
  type SkillDetail,
  type SkillImportResult,
} from "../../stores/skills";
import { useChatStore } from "../../stores/chat";

interface SkillsPageProps {
  onBack: () => void;
}

interface SkillsPanelProps {
  onBack?: () => void;
  initialSkillId?: string | null;
  onReviewEnabled?: (id: string) => void;
}

type Tab = "installed" | "marketplace";
type ReviewRefreshState = null | "refreshing" | "success" | "failed";

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

export function SkillsPage({ onBack }: SkillsPageProps) {
  return <SkillsPanel onBack={onBack} />;
}

export function SkillsPanel({ onBack, initialSkillId, onReviewEnabled }: SkillsPanelProps) {
  const { skills, loading, catalogError, loadSkills, enableSkill, disableSkill, installFromUrl, installMarketplace, selectSourceDirectory, importFromDirectory, createSkill, updateSkill, deleteSkill, getSkillDetail } =
    useSkillsStore();

  const [tab, setTab] = useState<Tab>("installed");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const detailRequestRef = useRef(0);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [installUrl, setInstallUrl] = useState("");
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<string | null>(null);
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [reviewingInstalledId, setReviewingInstalledId] = useState<string | null>(null);
  const [confirmingEnable, setConfirmingEnable] = useState<SkillDetail | null>(null);
  const [enableError, setEnableError] = useState<string | null>(null);
  const [enableReviewRefresh, setEnableReviewRefresh] = useState<ReviewRefreshState>(null);
  const [detailRefreshWarning, setDetailRefreshWarning] = useState<string | null>(null);
  const [detailActionError, setDetailActionError] = useState<string | null>(null);
  const [importResult, setImportResult] = useState<SkillImportResult | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = useState<SkillManifest | null>(null);
  // P2: propose skills from the active project's recurring task patterns.
  const proposeCwd = useChatStore((s) => s.activeSession?.cwd ?? null);
  const [proposing, setProposing] = useState(false);

  const handlePropose = async () => {
    if (!proposeCwd || proposing) return;
    setProposing(true);
    setInstallError(null);
    try {
      const created = await invoke<SkillManifest[]>("propose_skills_from_patterns", { cwd: proposeCwd });
      if (created.length === 0) {
        setInstallError("没有发现足够反复的任务模式（需 ≥4 次相似任务）");
      } else {
        await revealInstalledSkill(created[0]);
        try {
          await loadSkills();
        } catch (refreshError) {
          setInstallError(`提议已创建并保持未启用，但目录刷新失败：${String(refreshError)}`);
        }
      }
    } catch (e) {
      setInstallError(String(e));
    } finally {
      setProposing(false);
    }
  };

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
    const operationRequestId = detailRequestRef.current;
    setSavingForm(true);
    setFormError(null);
    try {
      if (form.mode === "create") {
        const created = await createSkill(form.name.trim(), form.description.trim(), form.instructions);
        setForm(null);
        await revealInstalledSkill(created);
      } else if (form.id) {
        await updateSkill(form.id, {
          name: form.name.trim(),
          description: form.description.trim(),
          instructions: form.instructions,
        });
        setForm(null);
        if (selectedId === form.id && operationRequestId === detailRequestRef.current) {
          setReviewingInstalledId(form.id);
          setDetail((current) => {
            if (!current || current.manifest.id !== form.id) return current;
            return {
              ...current,
              manifest: {
                ...current.manifest,
                name: form.name.trim(),
                description: form.description.trim(),
                enabled: false,
              },
              system_prompt: form.instructions,
            };
          });
          setDetailRefreshWarning(null);
          const requestId = ++detailRequestRef.current;
          try {
            const latest = await getSkillDetail(form.id);
            if (requestId === detailRequestRef.current) setDetail(latest);
          } catch (refreshError) {
            if (requestId === detailRequestRef.current) {
              setDetailRefreshWarning(`已保存为未启用版本，但详情刷新失败：${String(refreshError)}`);
            }
          }
        }
      }
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
    void loadSkills().catch(() => undefined);
  }, []);

  useEffect(() => {
    if (tab === "marketplace" && marketSkills.length === 0 && !marketLoading) {
      loadMarketplace();
    }
  }, [tab]);

  const handleImportOpenClaw = async () => {
    setInstallError(null);
    setInstalling(true);
    try {
      const found = await invoke<
        { name: string; description: string; path: string; source_handle: string; already_installed: boolean }[]
      >("scan_openclaw_skills");
      const fresh = found.filter((skill) => !skill.already_installed);
      if (found.length === 0) {
        setInstallError("没有在 ~/.openclaw 或 ~/.claude 的技能目录里发现可导入的技能。");
        return;
      }
      if (fresh.length === 0) {
        setInstallError(`发现 ${found.length} 个 OpenClaw 技能,均已导入过。`);
        return;
      }
      const combined: SkillImportResult = { succeeded: [], failed: [] };
      for (const skill of fresh) {
        try {
          const result = await importFromDirectory(skill.source_handle);
          combined.succeeded.push(...result.succeeded);
          combined.failed.push(...result.failed);
        } catch (error) {
          combined.failed.push({ path: skill.path, error: String(error) });
        }
      }
      await presentImportResult(combined);
    } catch (e) {
      setInstallError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const handleImportDir = async () => {
    setInstallError(null);
    try {
      const selection = await selectSourceDirectory();
      if (!selection) return;
      setInstalling(true);
      await presentImportResult(await importFromDirectory(selection.source_handle));
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
      const skills = await invoke<MarketplaceSkill[]>("fetch_marketplace_skills", {
        registryUrl: null,
      });
      setMarketSkills(skills);
      // Phase 0 containment deliberately exposes only the catalog embedded in
      // the signed app until registry signatures and package digests ship.
      setUsingLocalCatalog(true);
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
    setMarketError(null);
    try {
      const installed = await installMarketplace(skill.id);
      setMarketSkills((current) => current.map((item) => (
        item.id === skill.id ? { ...item, installed: true } : item
      )));
      await revealInstalledSkill(installed);
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
    const requestId = ++detailRequestRef.current;
    setSelectedId(id);
    setDetail(null);
    setDetailError(null);
    setDetailRefreshWarning(null);
    setDetailActionError(null);
    setDetailLoading(true);
    try {
      const d = await getSkillDetail(id);
      if (requestId === detailRequestRef.current) setDetail(d);
    } catch (e) {
      if (requestId === detailRequestRef.current) setDetailError(String(e));
    } finally {
      if (requestId === detailRequestRef.current) setDetailLoading(false);
    }
  };

  useEffect(() => {
    if (!initialSkillId) return;
    setTab("installed");
    setReviewingInstalledId(initialSkillId);
    void handleSelectSkill(initialSkillId);
  }, [initialSkillId]);

  const revealInstalledSkill = async (skill: SkillManifest) => {
    setTab("installed");
    setReviewingInstalledId(skill.id);
    await handleSelectSkill(skill.id);
  };

  const presentImportResult = async (result: SkillImportResult) => {
    setImportResult(result);
    setInstallError(null);
    if (result.succeeded[0]) {
      await revealInstalledSkill(result.succeeded[0]);
    }
  };

  const handleToggle = async (detailToToggle: SkillDetail) => {
    const skill = detailToToggle.manifest;
    if (!skill.enabled) {
      setEnableError(null);
      setEnableReviewRefresh(null);
      setConfirmingEnable(detailToToggle);
      return;
    }
    const operationRequestId = detailRequestRef.current;
    setTogglingId(skill.id);
    setDetailActionError(null);
    setDetailRefreshWarning(null);
    try {
      await disableSkill(skill.id);
      if (selectedId === skill.id && operationRequestId === detailRequestRef.current) {
        setDetail({
          ...detailToToggle,
          manifest: { ...skill, enabled: false },
        });
        const requestId = ++detailRequestRef.current;
        try {
          const latest = await getSkillDetail(skill.id);
          if (requestId === detailRequestRef.current) setDetail(latest);
        } catch (refreshError) {
          if (requestId === detailRequestRef.current) {
            setDetailRefreshWarning(`已禁用，但详情刷新失败：${String(refreshError)}`);
          }
        }
      }
    } catch (error) {
      if (operationRequestId === detailRequestRef.current) {
        setDetailActionError(`禁用失败：${String(error)}`);
      }
    } finally {
      setTogglingId(null);
    }
  };

  const refreshEnableReviewDetail = async (id: string) => {
    const requestId = ++detailRequestRef.current;
    setEnableReviewRefresh("refreshing");
    try {
      const latest = await getSkillDetail(id);
      if (requestId !== detailRequestRef.current) return;
      setDetail(latest);
      setConfirmingEnable(latest);
      setReviewingInstalledId(id);
      setEnableReviewRefresh("success");
    } catch {
      if (requestId === detailRequestRef.current) setEnableReviewRefresh("failed");
    }
  };

  const handleConfirmedEnable = async () => {
    const reviewed = confirmingEnable;
    if (!reviewed) return;
    const skill = reviewed.manifest;
    if (!reviewed.review_fingerprint) {
      setEnableError("SKILL_REVIEW_REQUIRED: 当前内容没有可批准的审核摘要，请重新打开详情");
      return;
    }
    const operationRequestId = detailRequestRef.current;
    setTogglingId(skill.id);
    try {
      await enableSkill(skill.id, reviewed.review_fingerprint);
      if (initialSkillId === skill.id && onReviewEnabled) {
        onReviewEnabled(skill.id);
        return;
      }
      setReviewingInstalledId(null);
      setConfirmingEnable(null);
      setEnableReviewRefresh(null);
      if (selectedId === skill.id && operationRequestId === detailRequestRef.current) {
        setDetail({ ...reviewed, manifest: { ...skill, enabled: true } });
        setDetailRefreshWarning(null);
        const requestId = ++detailRequestRef.current;
        try {
          const latest = await getSkillDetail(skill.id);
          if (requestId === detailRequestRef.current) setDetail(latest);
        } catch (refreshError) {
          if (requestId === detailRequestRef.current) {
            setDetailRefreshWarning(`已启用，但详情刷新失败：${String(refreshError)}`);
          }
        }
      }
    } catch (error) {
      const message = String(error);
      if (operationRequestId === detailRequestRef.current) setEnableError(message);
      if (
        operationRequestId === detailRequestRef.current
        && message.includes("SKILL_REVIEW_CONTENT_CHANGED")
        && selectedId === skill.id
      ) {
        await refreshEnableReviewDetail(skill.id);
      }
    } finally {
      setTogglingId(null);
    }
  };

  const handleDelete = async (id: string) => {
    setDeletingId(id);
    setDeleteError(null);
    try {
      await deleteSkill(id);
      setConfirmingDelete(null);
      if (selectedId === id) {
        detailRequestRef.current += 1;
        setSelectedId(null);
        setDetail(null);
      }
    } catch (error) {
      setDeleteError(`移除失败：${String(error)}`);
    } finally {
      setDeletingId(null);
    }
  };

  const handleInstall = async () => {
    if (!installUrl.trim()) return;
    setInstalling(true);
    setInstallError(null);
    try {
      const installed = await installFromUrl(installUrl.trim());
      setInstallUrl("");
      await revealInstalledSkill(installed);
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
          {onBack && (
            <button
              onClick={onBack}
              className="p-1 rounded text-gray-600 hover:text-gray-300 hover:bg-surface-3 transition-colors"
              title="返回"
            >
              <ChevronLeft size={14} />
            </button>
          )}
          <span className="flex-1 text-label font-semibold text-gray-400">
            技能
          </span>
        </div>

        {/* Tab switcher */}
        <div className="flex border-b border-border">
          <button
            onClick={() => setTab("installed")}
            className={`flex-1 py-1.5 text-label font-medium transition-colors ${
              tab === "installed"
                ? "text-gray-200 border-b-2 border-accent"
                : "text-gray-600 hover:text-gray-400"
            }`}
          >
            已安装
          </button>
          <button
            onClick={() => setTab("marketplace")}
            className={`flex-1 py-1.5 text-label font-medium flex items-center justify-center gap-1 transition-colors ${
              tab === "marketplace"
                ? "text-gray-200 border-b-2 border-accent"
                : "text-gray-600 hover:text-gray-400"
            }`}
          >
            <Store size={14} />
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
                  className="flex-1 bg-surface-3 border border-border rounded px-2 py-1 text-label text-gray-300 placeholder-gray-600 outline-none focus:border-accent/50"
                />
                <button
                  onClick={handleInstall}
                  disabled={installing || !installUrl.trim()}
                  className="p-1.5 rounded bg-accent hover:bg-accent-hover text-white disabled:opacity-50 transition-colors"
                  title="从 URL 安装技能"
                >
                  <Plus size={14} />
                </button>
                <button
                  onClick={handleImportDir}
                  disabled={installing}
                  className="p-1.5 rounded bg-surface-3 hover:bg-surface-2 text-gray-300 border border-border disabled:opacity-50 transition-colors"
                  title="从本地目录导入（支持 SKILL.md，如 superpowers / openspec，可整个仓库批量导入）"
                >
                  <FolderOpen size={14} />
                </button>
                <button
                  onClick={handleImportOpenClaw}
                  disabled={installing}
                  className="p-1.5 rounded bg-surface-3 hover:bg-surface-2 text-gray-300 border border-border disabled:opacity-50 transition-colors"
                  title="一键导入 OpenClaw 技能（自动扫描 ~/.openclaw/workspace/skills 与 ~/.claude/skills，已导入的自动跳过）"
                  aria-label="一键导入 OpenClaw 技能"
                >
                  <Download size={14} />
                </button>
              </div>
              <button
                onClick={() => {
                  setFormError(null);
                  setForm({ mode: "create", name: "", description: "", instructions: "" });
                }}
                className="w-full flex items-center justify-center gap-1 px-2 py-1 rounded bg-accent/15 hover:bg-accent/25 text-accent text-label transition-colors"
              >
                <Plus size={14} /> 新建技能
              </button>
              <button
                onClick={handlePropose}
                disabled={!proposeCwd || proposing}
                title={
                  proposeCwd
                    ? "从当前项目反复出现的任务模式里提议一个技能（生成禁用草稿，预览/编辑后再启用）"
                    : "在某个项目会话里打开技能库时可用"
                }
                className="w-full flex items-center justify-center gap-1 px-2 py-1 rounded bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-700 dark:text-emerald-300 text-label transition-colors disabled:opacity-50"
              >
                {proposing ? <Loader2 size={14} className="animate-spin" /> : <Sparkles size={14} />}
                从使用习惯提议技能
              </button>
              {installError && (
                <p className="text-label text-red-400 whitespace-pre-wrap break-words" title={installError}>
                  {installError}
                </p>
              )}
              {catalogError && (
                <div className="space-y-1 text-label text-red-700 dark:text-red-300" role="alert">
                  <p className="whitespace-pre-wrap break-words">{catalogError}</p>
                  <button
                    className="rounded bg-surface-3 px-2 py-1 text-label text-gray-300 hover:bg-surface-4"
                    onClick={() => void loadSkills().catch(() => undefined)}
                  >
                    重试加载技能目录
                  </button>
                </div>
              )}
              {importResult && (
                <div
                  className="text-label text-amber-400 whitespace-pre-wrap break-words"
                  role="status"
                  aria-label="批量导入结果"
                >
                  <p>成功 {importResult.succeeded.length} 个，失败 {importResult.failed.length} 个。</p>
                  {importResult.succeeded.map((skill) => (
                    <button
                      key={`ok-${skill.id}`}
                      className="block w-full rounded px-1 py-0.5 text-left underline-offset-2 hover:bg-surface-3 hover:underline focus-visible:outline focus-visible:outline-1 focus-visible:outline-accent"
                      aria-label={`检查已安装的 ${skill.id}`}
                      onClick={() => void revealInstalledSkill(skill)}
                    >
                      已安装：{skill.id}（未启用）
                    </button>
                  ))}
                  {importResult.failed.map((failure) => (
                    <p key={`failed-${failure.path}`}>失败：{failure.path} — {failure.error}</p>
                  ))}
                </div>
              )}
            </div>

            {/* Installed skill list */}
            <ul className="flex-1 overflow-y-auto py-1">
              {loading && (
                <li className="px-3 py-2 text-label text-gray-700">加载中…</li>
              )}
              {!loading && !catalogError && skills.length === 0 && (
                <li className="px-3 py-2 text-label text-gray-700">未安装任何技能</li>
              )}
              {skills.map((skill) => (
                <li key={skill.id} className="flex items-stretch">
                  <button
                    aria-label={`查看 ${skill.name}`}
                    className={`min-w-0 flex-1 flex flex-col gap-0.5 px-3 py-2 text-left transition-colors ${
                      selectedId === skill.id
                        ? "bg-surface-3 text-gray-200"
                        : "text-gray-500 hover:bg-surface-2 hover:text-gray-300"
                    } ${skill.enabled ? "border-l-2 border-accent" : "border-l-2 border-transparent"}`}
                    onClick={() => handleSelectSkill(skill.id)}
                  >
                    <div className="flex items-center gap-1.5 w-full min-w-0">
                      <span className="flex-1 truncate text-label font-medium">
                        {skill.name}
                      </span>
                    </div>
                    <div className="flex items-center gap-1 flex-wrap">
                      {skill.lifecycle_status === "corrupt" && (
                        <span className="px-1 py-0.5 rounded bg-red-500/15 text-red-400 text-caption">
                          损坏，未启用
                        </span>
                      )}
                      {skill.lifecycle_status !== "corrupt" && skill.enabled && (
                        <span className="px-1 py-0.5 rounded bg-accent/20 text-accent text-caption">
                          已启用
                        </span>
                      )}
                      {skill.lifecycle_status !== "corrupt" && !skill.enabled && (
                        <span className="px-1 py-0.5 rounded bg-surface-3 text-gray-500 text-caption">
                          未启用
                        </span>
                      )}
                      <span className="text-caption text-gray-700">
                        {skill.source}
                      </span>
                    </div>
                  </button>
                  {skill.source === "user" && (
                    <button
                      className="flex-shrink-0 px-2 text-gray-600 hover:bg-surface-3 hover:text-red-400 focus-visible:text-red-400 disabled:opacity-50"
                      aria-label={`删除 ${skill.name}`}
                      title="删除"
                      disabled={deletingId === skill.id}
                      onClick={() => {
                        setDeleteError(null);
                        setConfirmingDelete(skill);
                      }}
                    >
                      {deletingId === skill.id ? <Loader2 size={14} className="animate-spin" /> : <Trash2 size={14} />}
                    </button>
                  )}
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
                className="w-full bg-surface-3 border border-border rounded px-2 py-1 text-label text-gray-300 placeholder-gray-600 outline-none focus:border-accent/50"
              />
            </div>

            {/* Marketplace list */}
            <ul className="flex-1 overflow-y-auto py-1">
              {marketLoading && (
                <li className="px-3 py-2 text-label text-gray-700">正在加载目录…</li>
              )}
              {!marketLoading && marketError && (
                <li className="px-3 py-2 text-label text-red-400">{marketError}</li>
              )}
              {!marketLoading && !marketError && filteredMarketSkills.length === 0 && (
                <li className="px-3 py-2 text-label text-gray-700">未找到技能</li>
              )}
              {filteredMarketSkills.map((skill) => (
                <li key={skill.id} className="px-3 py-2 border-b border-border/50">
                  <div className="flex items-start gap-2">
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-1.5">
                        <span className="text-label font-medium text-gray-300 truncate">
                          {skill.name}
                        </span>
                        {skill.installed && (
                          <span className="px-1 py-0.5 rounded bg-accent/20 text-accent text-caption flex-shrink-0">
                            已安装
                          </span>
                        )}
                      </div>
                      <p className="text-caption text-gray-600 mt-0.5 line-clamp-2">
                        {skill.description}
                      </p>
                      <div className="flex gap-1 mt-1 flex-wrap">
                        {skill.tags.map((tag) => (
                          <span
                            key={tag}
                            className="flex items-center gap-0.5 px-1 py-0.5 rounded bg-surface-3 text-gray-500 text-caption"
                          >
                            <Tag size={14} />
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
                          <span className="text-caption">...</span>
                        ) : (
                          <Download size={14} />
                        )}
                      </button>
                    )}
                  </div>
                </li>
              ))}
            </ul>

            {/* Local catalog notice */}
            {usingLocalCatalog && !marketLoading && (
              <div className="px-3 py-1.5 border-t border-border text-caption text-gray-700">
                安全止血期间仅使用随应用提供的离线目录
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
          <div className="flex-1 flex items-center justify-center text-body text-gray-700">
            选择一个技能查看详情
          </div>
        ) : detailLoading ? (
          <div className="flex-1 flex items-center justify-center text-body text-gray-700">
            加载中…
          </div>
        ) : detailError ? (
          <div className="flex-1 flex flex-col items-center justify-center gap-3 px-6 text-center">
            <div className="text-label text-red-400 whitespace-pre-wrap" role="alert">
              Skill “{selectedId}” 的安装收据存在，但当前状态无法确认：{detailError}
            </div>
            <button
              className="rounded bg-surface-3 px-3 py-1.5 text-label text-gray-300 hover:bg-surface-4"
              onClick={() => selectedId && handleSelectSkill(selectedId)}
            >
              重试加载
            </button>
          </div>
        ) : detail ? (
          <SkillDetailView
            detail={detail}
            reviewing={reviewingInstalledId === detail.manifest.id}
            toggling={togglingId === detail.manifest.id}
            deleting={deletingId === detail.manifest.id}
            deleteError={deleteError}
            refreshWarning={detailRefreshWarning}
            actionError={detailActionError}
            onRetryDetail={() => void handleSelectSkill(detail.manifest.id)}
            onToggle={() => handleToggle(detail)}
            onDelete={() => setConfirmingDelete(detail.manifest)}
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
      {confirmingEnable && (
        <EnableSkillDialog
          skill={confirmingEnable.manifest}
          saving={togglingId === confirmingEnable.manifest.id}
          error={enableError}
          reviewRefresh={enableReviewRefresh}
          onReloadDetail={() => void refreshEnableReviewDetail(confirmingEnable.manifest.id)}
          onCancel={() => {
            if (togglingId !== confirmingEnable.manifest.id) {
              detailRequestRef.current += 1;
              setConfirmingEnable(null);
              setEnableReviewRefresh(null);
            }
          }}
          onConfirm={handleConfirmedEnable}
        />
      )}
      {confirmingDelete && (
        <DeleteSkillDialog
          skill={confirmingDelete}
          deleting={deletingId === confirmingDelete.id}
          error={deleteError}
          onCancel={() => {
            if (deletingId !== confirmingDelete.id) setConfirmingDelete(null);
          }}
          onConfirm={() => void handleDelete(confirmingDelete.id)}
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
  const dialogRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    return () => previousFocusRef.current?.focus();
  }, []);

  useEffect(() => {
    if (saving) dialogRef.current?.focus();
  }, [saving]);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      if (!saving) onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    if (saving) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }
    const focusable = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(
      "button:not([disabled]), input:not([disabled]), textarea:not([disabled])",
    ) ?? []);
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={() => {
        if (!saving) onCancel();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-busy={saving}
        aria-labelledby="skill-form-title"
        tabIndex={-1}
        className="w-full max-w-lg rounded-lg border border-border bg-surface-1 shadow-2xl flex flex-col max-h-[85vh]"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <header className="flex items-center justify-between px-4 py-3 border-b border-border">
          <h2 id="skill-form-title" className="text-body font-semibold text-gray-200">
            {form.mode === "create" ? "新建技能" : "编辑技能"}
          </h2>
          <button
            onClick={onCancel}
            disabled={saving}
            aria-label="关闭"
            className="p-1 rounded text-gray-500 hover:text-gray-200 hover:bg-surface-3 disabled:opacity-40"
          >
            <X size={14} />
          </button>
        </header>
        <fieldset disabled={saving} className="flex-1 min-w-0 overflow-y-auto p-4 space-y-3">
          <div>
            <label className="block text-caption text-gray-500 mb-1">名称</label>
            <input
              type="text"
              autoFocus
              value={form.name}
              onChange={(e) => onChange({ ...form, name: e.target.value })}
              placeholder="例如 周报助手"
              className="w-full bg-surface-2 border border-border rounded px-2 py-1.5 text-body text-gray-200 outline-none focus:border-accent"
            />
          </div>
          <div>
            <label className="block text-caption text-gray-500 mb-1">描述（何时使用）</label>
            <input
              type="text"
              value={form.description}
              onChange={(e) => onChange({ ...form, description: e.target.value })}
              className="w-full bg-surface-2 border border-border rounded px-2 py-1.5 text-body text-gray-200 outline-none focus:border-accent"
            />
          </div>
          <div>
            <label className="block text-caption text-gray-500 mb-1">技能指令（启用后注入系统提示）</label>
            <textarea
              value={form.instructions}
              onChange={(e) => onChange({ ...form, instructions: e.target.value })}
              rows={10}
              placeholder="用自然语言写下这个技能要让 AI 怎么做…"
              className="w-full bg-surface-2 border border-border rounded px-2 py-1.5 text-note text-gray-200 outline-none focus:border-accent resize-y font-mono leading-relaxed"
            />
          </div>
          {error && <p className="text-label text-red-400 break-words">{error}</p>}
        </fieldset>
        <footer className="flex items-center justify-end gap-2 px-4 py-3 border-t border-border">
          <button
            onClick={onCancel}
            disabled={saving}
            className="px-3 py-1.5 rounded text-label text-gray-400 hover:bg-surface-3 disabled:opacity-40"
          >
            取消
          </button>
          <button
            onClick={onSave}
            disabled={saving || !form.name.trim()}
            className="px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-label disabled:opacity-40"
          >
            {saving ? "保存中…" : form.mode === "create" ? "创建为未启用" : "保存为未启用版本"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function MarketplaceWelcome() {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-3 text-center px-8">
      <Store size={24} className="text-gray-700" />
      <div>
        <h2 className="text-body font-medium text-gray-400">技能市场</h2>
        <p className="text-label text-gray-700 mt-1 max-w-xs">
          浏览随 CodeFactory 提供的离线技能目录。安装后会自动打开实际内容，确认后再启用。
        </p>
      </div>
    </div>
  );
}

function SkillDetailView({
  detail,
  reviewing,
  toggling,
  deleting,
  deleteError,
  refreshWarning,
  actionError,
  onRetryDetail,
  onToggle,
  onDelete,
  onEdit,
}: {
  detail: SkillDetail;
  reviewing: boolean;
  toggling: boolean;
  deleting: boolean;
  deleteError: string | null;
  refreshWarning: string | null;
  actionError: string | null;
  onRetryDetail: () => void;
  onToggle: () => void;
  onDelete: () => void;
  onEdit: () => void;
}) {
  const { manifest, system_prompt, slash_commands, has_tool_policy, tool_policy } = detail;
  const isCorrupt = manifest.lifecycle_status === "corrupt";
  const reviewStatusRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (reviewing && !manifest.enabled && !isCorrupt) reviewStatusRef.current?.focus();
  }, [isCorrupt, manifest.enabled, manifest.id, reviewing]);

  return (
    <div className="flex flex-col h-full overflow-y-auto p-5 gap-5">
      {refreshWarning && (
        <div className="rounded border border-yellow-500/30 bg-yellow-500/10 px-3 py-2 text-label text-yellow-700 dark:text-yellow-300" role="alert">
          <p>{refreshWarning}</p>
          <button className="mt-2 rounded bg-surface-3 px-2 py-1 text-gray-300 hover:bg-surface-4" onClick={onRetryDetail}>
            重新加载详情
          </button>
        </div>
      )}
      {actionError && (
        <div className="rounded border border-red-500/30 bg-red-500/10 px-3 py-2 text-label text-red-700 dark:text-red-300" role="alert">
          {actionError}
        </div>
      )}
      {reviewing && !manifest.enabled && !isCorrupt && (
        <div
          ref={reviewStatusRef}
          className="rounded border border-accent/30 bg-accent/10 px-3 py-2 text-label text-gray-300"
          role="status"
          aria-label="安装结果"
          tabIndex={-1}
        >
          已安装，尚未启用。检查内容后再启用。
        </div>
      )}
      {isCorrupt && (
        <div
          className="space-y-2 rounded border border-red-500/30 bg-red-500/10 px-3 py-2 text-label text-red-700 dark:text-red-300"
          role="alert"
        >
          <p>此 Skill 的安装目录存在，但 manifest 缺失、损坏或与目录 ID 不一致。它不会进入任何任务上下文；请移除后从可信来源重新安装。</p>
          {manifest.source === "user" && (
            <button
              className="rounded bg-red-700 px-2 py-1 text-white hover:bg-red-800 disabled:opacity-50 dark:bg-red-600 dark:hover:bg-red-500"
              disabled={deleting}
              onClick={onDelete}
            >
              {deleting ? "移除中…" : "移除此损坏项…"}
            </button>
          )}
          {deleteError && <p className="whitespace-pre-wrap">{deleteError}</p>}
        </div>
      )}
      {/* Header */}
      <div className="flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <h1 className="text-body font-semibold text-gray-200">{manifest.name}</h1>
          <p className="text-label text-gray-500 mt-0.5">{manifest.description}</p>
          <div className="flex items-center gap-2 mt-1 flex-wrap">
            <span className="text-caption text-gray-700">v{manifest.version} 作者 {manifest.author}</span>
            {manifest.tags.map((tag) => (
              <span
                key={tag}
                className="flex items-center gap-0.5 px-1.5 py-0.5 rounded bg-surface-3 text-gray-400 text-caption"
              >
                <Tag size={14} />
                {tag}
              </span>
            ))}
            {has_tool_policy && (
              <span className="px-1.5 py-0.5 rounded bg-yellow-900/40 text-yellow-400 text-caption">
                工具策略（当前未接入运行时）
              </span>
            )}
          </div>
        </div>
        {manifest.source === "user" && !isCorrupt && (
          <button
            onClick={onEdit}
            className="px-3 py-1.5 rounded text-label font-medium bg-surface-3 text-gray-400 hover:text-gray-200 hover:bg-surface-4 transition-colors"
          >
            编辑
          </button>
        )}
        {!isCorrupt && (
          <button
            onClick={onToggle}
            disabled={toggling}
            className={`px-3 py-1.5 rounded text-label font-medium transition-colors disabled:opacity-50 ${
              manifest.enabled
                ? "bg-surface-3 text-gray-400 hover:text-red-400 hover:bg-surface-4"
                : "bg-accent hover:bg-accent-hover text-white"
            }`}
          >
            {toggling ? "…" : manifest.enabled ? "禁用" : "检查并启用…"}
          </button>
        )}
      </div>

      {/* System prompt */}
      <div>
        <h2 className="text-label font-semibold text-gray-500 mb-2">
          系统提示词
        </h2>
        <pre className="text-label text-gray-300 bg-surface-1 border border-border rounded p-3 whitespace-pre-wrap leading-relaxed font-mono">
          {system_prompt || "（无）"}
        </pre>
      </div>

      {/* Slash commands */}
      {slash_commands.length > 0 && (
        <div>
          <h2 className="text-label font-semibold text-gray-500 mb-2">
            斜杠命令（当前未接入运行时）
          </h2>
          <div className="space-y-1.5">
            {slash_commands.map((cmd) => (
              <div
                key={cmd.name}
                className="flex items-start gap-3 rounded border border-border bg-surface-1 px-3 py-2"
              >
                <div className="flex items-center gap-1 flex-shrink-0">
                  <Terminal size={14} className="text-accent" />
                  <code className="text-label text-accent font-mono">/{cmd.name}</code>
                </div>
                <div className="flex-1 min-w-0">
                  <div className="text-label text-gray-300">{cmd.description}</div>
                  <div className="text-caption text-gray-600 mt-0.5 font-mono truncate">
                    {cmd.template}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {tool_policy && (
        <div>
          <h2 className="text-label font-semibold text-gray-500 mb-2">
            工具策略声明（当前仅供审核，未接入运行时）
          </h2>
          <pre className="text-label text-gray-300 bg-surface-1 border border-border rounded p-3 whitespace-pre-wrap leading-relaxed font-mono">
            {tool_policy}
          </pre>
        </div>
      )}
    </div>
  );
}

function EnableSkillDialog({
  skill,
  saving,
  error,
  reviewRefresh,
  onReloadDetail,
  onCancel,
  onConfirm,
}: {
  skill: SkillManifest;
  saving: boolean;
  error: string | null;
  reviewRefresh: ReviewRefreshState;
  onReloadDetail: () => void;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const contentChanged = error?.includes("SKILL_REVIEW_CONTENT_CHANGED") ?? false;

  useEffect(() => {
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    confirmRef.current?.focus();
    return () => previousFocusRef.current?.focus();
  }, []);

  useEffect(() => {
    if (saving) dialogRef.current?.focus();
  }, [saving]);

  const handleDialogKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      if (!saving) onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    if (saving) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }
    const first = cancelRef.current;
    const last = confirmRef.current;
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={() => {
        if (!saving) onCancel();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-busy={saving}
        aria-labelledby="enable-skill-title"
        tabIndex={-1}
        className="w-full max-w-md rounded-lg border border-border bg-surface-1 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={handleDialogKeyDown}
      >
        <header className="px-4 py-3 border-b border-border">
          <h2 id="enable-skill-title" className="text-body font-semibold text-gray-200">
            启用“{skill.name}”？
          </h2>
        </header>
        <div className="p-4 space-y-2 text-label text-gray-400">
          <p>当前版本将对所有项目生效；仅当前项目范围将在 Skill v2 的作用域阶段提供。</p>
          <p>启用后会从下一次新任务起进入现有 Skill 上下文；这不表示每个任务都会实际使用它。</p>
          {error && (
            <p className="text-red-400 whitespace-pre-wrap" role="alert">
              启用失败：{error}
            </p>
          )}
          {contentChanged && reviewRefresh === "success" && (
            <p>已在后台刷新为最新内容。请取消此确认框，重新检查详情后再启用。</p>
          )}
          {contentChanged && reviewRefresh === "failed" && (
            <div>
              <p>最新内容加载失败；当前页面仍是旧版本，不能继续批准。</p>
              <button
                className="mt-2 rounded bg-surface-3 px-2 py-1 text-gray-300 hover:bg-surface-4"
                onClick={onReloadDetail}
              >
                重新加载最新详情
              </button>
            </div>
          )}
          {contentChanged && reviewRefresh === "refreshing" && <p>正在加载最新内容…</p>}
        </div>
        <footer className="flex items-center justify-end gap-2 px-4 py-3 border-t border-border">
          <button
            ref={cancelRef}
            onClick={onCancel}
            disabled={saving}
            className="px-3 py-1.5 rounded text-label text-gray-400 hover:bg-surface-3 disabled:opacity-50"
          >
            取消
          </button>
          <button
            ref={confirmRef}
            onClick={onConfirm}
            disabled={saving || contentChanged}
            className="px-3 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-label disabled:opacity-50"
          >
            {saving ? "启用中…" : "批准并在所有项目启用"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function DeleteSkillDialog({
  skill,
  deleting,
  error,
  onCancel,
  onConfirm,
}: {
  skill: SkillManifest;
  deleting: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    cancelRef.current?.focus();
    return () => previousFocusRef.current?.focus();
  }, []);

  useEffect(() => {
    if (deleting) dialogRef.current?.focus();
  }, [deleting]);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      if (!deleting) onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    if (deleting) {
      event.preventDefault();
      dialogRef.current?.focus();
      return;
    }
    if (event.shiftKey && document.activeElement === cancelRef.current) {
      event.preventDefault();
      confirmRef.current?.focus();
    } else if (!event.shiftKey && document.activeElement === confirmRef.current) {
      event.preventDefault();
      cancelRef.current?.focus();
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={() => {
        if (!deleting) onCancel();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-busy={deleting}
        aria-labelledby="delete-skill-title"
        tabIndex={-1}
        className="w-full max-w-md rounded-lg border border-border bg-surface-1 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <header className="px-4 py-3 border-b border-border">
          <h2 id="delete-skill-title" className="text-body font-semibold text-gray-200">
            永久移除“{skill.name}”？
          </h2>
        </header>
        <div className="p-4 space-y-2 text-label text-gray-400">
          <p>只会删除 CodeFactory 已安装的副本，不会修改原始 URL、本地目录或 OpenClaw 来源。</p>
          <p>Phase 0 暂不支持废纸篓恢复；确认后此安装副本无法在 CodeFactory 内撤销。</p>
          {error && (
            <p className="text-red-400 whitespace-pre-wrap" role="alert">
              {error}
            </p>
          )}
        </div>
        <footer className="flex items-center justify-end gap-2 px-4 py-3 border-t border-border">
          <button
            ref={cancelRef}
            onClick={onCancel}
            disabled={deleting}
            className="px-3 py-1.5 rounded text-label text-gray-400 hover:bg-surface-3 disabled:opacity-50"
          >
            取消
          </button>
          <button
            ref={confirmRef}
            onClick={onConfirm}
            disabled={deleting}
            className="px-3 py-1.5 rounded bg-red-700 hover:bg-red-800 text-white text-label disabled:opacity-50 dark:bg-red-600 dark:hover:bg-red-500"
          >
            {deleting ? "移除中…" : "永久移除已安装副本"}
          </button>
        </footer>
      </div>
    </div>
  );
}
