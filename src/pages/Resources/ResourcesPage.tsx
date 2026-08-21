// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import {
  BookOpen,
  ChevronLeft,
  FolderPlus,
  Loader2,
  Puzzle,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { SkillsPanel } from "../Skills/SkillsPage";
import { useKnowledgeStore } from "../../stores/knowledge";
import type { KnowledgeLibrary } from "../../lib/tauri";

interface ResourcesPageProps {
  onBack: () => void;
  initialTab?: ResourceTab;
  initialSkillId?: string | null;
  onSkillEnabled?: (id: string) => void;
}

type ResourceTab = "knowledge" | "skills";

export function ResourcesPage({ onBack, initialTab, initialSkillId, onSkillEnabled }: ResourcesPageProps) {
  const [tab, setTab] = useState<ResourceTab>(initialTab ?? (initialSkillId ? "skills" : "knowledge"));

  return (
    <div className="flex h-full flex-col bg-surface-0">
      <header className="flex items-center gap-3 border-b border-border bg-surface-1 px-4 py-3">
        <button
          onClick={onBack}
          className="rounded p-1 text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
          title="返回"
          aria-label="返回"
        >
          <ChevronLeft size={16} />
        </button>
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <Puzzle size={16} className="text-accent" />
          <div>
            <h1 className="text-body font-semibold text-gray-200">资源中心</h1>
            <p className="text-caption text-gray-600">统一管理 Agent 自动使用的知识库与技能</p>
          </div>
        </div>
        <nav className="flex rounded border border-border bg-surface-2 p-0.5" aria-label="资源类型" role="tablist">
          <TabButton active={tab === "knowledge"} onClick={() => setTab("knowledge")}>
            知识库
          </TabButton>
          <TabButton active={tab === "skills"} onClick={() => setTab("skills")}>
            技能
          </TabButton>
        </nav>
      </header>
      <main className="min-h-0 flex-1">
        {tab === "knowledge" ? (
          <KnowledgeLibrariesPanel />
        ) : (
          <SkillsPanel initialSkillId={initialSkillId} onReviewEnabled={onSkillEnabled} />
        )}
      </main>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      role="tab"
      aria-selected={active}
      className={`rounded px-3 py-1.5 text-label transition-colors ${
        active ? "bg-accent text-white" : "text-gray-500 hover:bg-surface-3 hover:text-gray-200"
      }`}
    >
      {children}
    </button>
  );
}

function KnowledgeLibrariesPanel() {
  const {
    libraries,
    scanSummaries,
    loading,
    scanning,
    error,
    loadLibraries,
    registerLibrary,
    scanLibrary,
    setLibraryEnabled,
    deleteLibrary,
  } = useKnowledgeStore();
  const [actionError, setActionError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    void loadLibraries();
  }, [loadLibraries]);

  const addLibrary = async () => {
    setActionError(null);
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "选择知识库文件夹",
      });
      if (typeof selected !== "string") return;
      const name = selected.split(/[\\/]/).filter(Boolean).pop() ?? "个人知识库";
      setAdding(true);
      await registerLibrary(name, selected);
    } catch (e) {
      setActionError(String(e));
    } finally {
      setAdding(false);
    }
  };

  const run = async (operation: () => Promise<unknown>) => {
    setActionError(null);
    try {
      await operation();
    } catch (e) {
      setActionError(String(e));
    }
  };

  const remove = async (library: KnowledgeLibrary) => {
    const confirmed = window.confirm(
      `删除知识库“${library.name}”？只会删除 CodeFactory 的索引和注册记录，不会删除源文件。`,
    );
    if (!confirmed) return;
    await run(() => deleteLibrary(library.id));
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-5xl space-y-5 px-6 py-6">
        <section className="flex items-start justify-between gap-4">
          <div>
            <h2 className="flex items-center gap-2 text-body font-semibold text-gray-200">
              <BookOpen size={16} className="text-accent" />
              个人知识库
            </h2>
            <p className="mt-1 text-label text-gray-600">
              启用的资料会自动供普通会话和新建自主任务检索；无需在 Session 中重复配置。
            </p>
          </div>
          <button
            onClick={() => void addLibrary()}
            disabled={adding}
            className="inline-flex items-center gap-1.5 rounded bg-accent px-3 py-1.5 text-label font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
            aria-label="添加知识库"
          >
            {adding ? <Loader2 size={14} className="animate-spin" /> : <FolderPlus size={14} />}
            添加知识库
          </button>
        </section>

        {(actionError || error) && (
          <div className="rounded border border-red-500/20 bg-red-500/10 px-3 py-2 text-label text-red-700 dark:text-red-300">
            {actionError || error}
          </div>
        )}

        {loading && libraries.length === 0 ? (
          <div className="py-16 text-center text-label text-gray-600">加载知识库…</div>
        ) : libraries.length === 0 ? (
          <button
            onClick={() => void addLibrary()}
            className="flex w-full flex-col items-center gap-2 rounded-lg border border-dashed border-border bg-surface-1 py-16 text-gray-600 transition-colors hover:bg-surface-2 hover:text-gray-300"
          >
            <FolderPlus size={24} />
            <span className="text-body">添加本地资料文件夹</span>
            <span className="text-caption">支持 DOCX、PPTX 和 PDF</span>
          </button>
        ) : (
          <ul className="grid gap-3 md:grid-cols-2">
            {libraries.map((library) => {
              const summary = scanSummaries[library.id];
              const isScanning = scanning[library.id] ?? false;
              return (
                <li key={library.id} className="rounded-lg border border-border bg-surface-1 p-4">
                  <div className="flex items-start gap-3">
                    <BookOpen size={16} className="mt-0.5 shrink-0 text-accent" />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <h3 className="truncate text-body font-medium text-gray-200">{library.name}</h3>
                        <span className={`rounded px-1.5 py-0.5 text-caption ${
                          library.enabled
                            ? "bg-emerald-500/15 text-emerald-700 dark:text-emerald-300"
                            : "bg-surface-3 text-gray-600"
                        }`}>
                          {library.enabled ? "已启用" : "已禁用"}
                        </span>
                      </div>
                      <p className="mt-1 truncate font-mono text-caption text-gray-600" title={library.root_path}>
                        {library.root_path}
                      </p>
                      <p className="mt-2 text-caption text-gray-500">
                        {summary
                          ? `${summary.indexed_documents} 文档 · ${summary.chunks_indexed} 片段 · ${summary.failed_documents} 失败`
                          : scanStatusText(library.scan_status)}
                      </p>
                      {library.last_scan_at && (
                        <p className="mt-0.5 text-caption text-gray-700">
                          最近扫描：{new Date(library.last_scan_at).toLocaleString()}
                        </p>
                      )}
                    </div>
                  </div>
                  <div className="mt-4 flex items-center gap-2 border-t border-border pt-3">
                    <button
                      onClick={() => void run(() => scanLibrary(library.id))}
                      disabled={isScanning}
                      aria-label={`扫描知识库 ${library.name}`}
                      className="inline-flex items-center gap-1 rounded bg-surface-3 px-2 py-1 text-caption text-gray-400 hover:text-gray-200 disabled:opacity-50"
                    >
                      <RefreshCw size={14} className={isScanning ? "animate-spin" : ""} />
                      {isScanning ? "扫描中" : "重新扫描"}
                    </button>
                    <button
                      onClick={() => void run(() => setLibraryEnabled(library.id, !library.enabled))}
                      aria-label={`${library.enabled ? "禁用" : "启用"}知识库 ${library.name}`}
                      className="rounded bg-surface-3 px-2 py-1 text-caption text-gray-400 hover:text-gray-200"
                    >
                      {library.enabled ? "禁用" : "启用"}
                    </button>
                    <button
                      onClick={() => void remove(library)}
                      aria-label={`删除知识库 ${library.name}`}
                      className="ml-auto inline-flex items-center gap-1 rounded px-2 py-1 text-caption text-red-700 hover:bg-red-500/10 hover:text-red-500"
                    >
                      <Trash2 size={14} /> 删除
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}

function scanStatusText(status: string): string {
  switch (status) {
    case "completed":
      return "索引完成";
    case "completed_with_errors":
      return "索引完成（部分文件失败）";
    case "scanning":
      return "正在扫描";
    case "idle":
      return "尚未扫描";
    default:
      return status || "未知状态";
  }
}
