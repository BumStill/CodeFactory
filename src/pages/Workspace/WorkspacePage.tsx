// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject, type PointerEvent as ReactPointerEvent } from "react";
import {
  AlertTriangle,
  BookOpen,
  Settings as SettingsIcon,
  MessageSquare,
  Circle,
  CheckCircle2,
  Loader2,
  XCircle,
  Square,
  EyeOff,
  X,
  Globe2,
  ExternalLink,
  FileText,
  PanelRightOpen,
} from "lucide-react";
import { MessageList } from "../../components/MessageList";
import { DocumentPreview, type DocumentTab } from "../../components/DocumentPreview";
import { MessageInput } from "../../components/MessageInput";
import { SessionSidebar } from "../../components/SessionSidebar";
import { DraftScopeBar } from "../../components/DraftScopeBar";
import { ModelPicker } from "../../components/ModelPicker";
import { ReasoningEffortPicker } from "../../components/ReasoningEffortPicker";
import { PermissionModePicker } from "../../components/PermissionModePicker";
import { PermissionDialog } from "../../components/PermissionDialog";
import { ContextUsageBar } from "../../components/ContextUsageBar";
import { GitStatusBar } from "../../components/GitStatusBar";
import { WorkspaceDeliveryStatus, type WorkspaceDeliveryState } from "../../components/WorkspaceDeliveryStatus";
import { GitChangesPanel } from "../../components/GitChangesPanel";
import { GitHistoryPanel } from "../../components/GitHistoryPanel";
import { RemoteGitPanel } from "../../components/RemoteGitPanel";
import { useGitStore } from "../../stores/git";
import { recentProjects } from "../../lib/projects";
import { invoke, onEmbeddedBrowserEscape } from "../../lib/tauri";
import { useChatStore, activeRuntime, type UIMessage } from "../../stores/chat";
import { QueueBadge } from "../../components/QueueBadge";
import { useTasksStore } from "../../stores/tasks";
import type { BrowserSession, TaskRun, VerificationResult } from "../../lib/tauri";
import type { ExternalJobState, TurnTimingProfile } from "../../lib/chatPlan";
import { parseVerification, verificationSummary } from "../../lib/verification";

type WorkspaceBrowserSession = BrowserSession & {
  status?: string | null;
  pane_url?: string | null;
  current_host?: string | null;
  page_title?: string | null;
};

type AuxiliaryPaneKind = "browser" | "document" | "git" | "tasks" | "delivery" | "evidence";
type AuxiliaryLayout = "dock" | "drawer" | "overlay";

const AUXILIARY_PANE_DEFAULT_WIDTH = 520;
const AUXILIARY_PANE_MIN_WIDTH = 480;
const AUXILIARY_PANE_MAX_WIDTH = 720;
const AUXILIARY_STATUS_PANE_WIDTH = 400;
const WORKSPACE_READING_MIN_WIDTH = 560;
const WORKSPACE_SIDEBAR_WIDTH = 272;
const AUXILIARY_PANE_STORAGE_KEY = "cf.workspace.auxiliaryPaneWidth";

function auxiliaryLayoutForWidth(width: number): AuxiliaryLayout {
  if (width >= 1440) return "dock";
  if (width >= 1024) return "drawer";
  return "overlay";
}

function clampAuxiliaryPaneWidth(
  width: number,
  maxWidth = AUXILIARY_PANE_MAX_WIDTH,
): number {
  return Math.min(maxWidth, Math.max(AUXILIARY_PANE_MIN_WIDTH, width));
}

function auxiliaryPaneLabel(kind: AuxiliaryPaneKind): string {
  switch (kind) {
    case "browser": return "浏览器";
    case "document": return "文档";
    case "git": return "Git";
    case "tasks": return "任务活动";
    case "delivery": return "交付详情";
    case "evidence": return "回合证据";
  }
}

function sessionHost(session: WorkspaceBrowserSession): string {
  if (session.current_host) return session.current_host;
  const url = session.pane_url;
  if (!url) return "受管浏览器";
  try {
    return new URL(url).host || "受管浏览器";
  } catch {
    return "受管浏览器";
  }
}

function sessionTitle(session: WorkspaceBrowserSession): string {
  return session.page_title || sessionHost(session);
}

interface WorkspacePageProps {
  sessionId: string;
  /** Start a blank conversation, optionally scoped to a project directory. */
  onNewConversation: (cwd?: string | null) => void;
  onOpenSettings: (tab?: "capabilities" | "endpoints") => void;
  onOpenUsage?: () => void;
  /** Switch the workspace to another session in-place (from the sidebar). */
  onOpenSession: (id: string) => void;
  /** Reveal the task workbench and highlight this task when deep-linked from usage. */
  initialTaskLogId?: string | null;
}

/**
 * Workspace — the primary working surface.
 *
 * Three-column layout:
 *   Left   — Session sidebar (Codex-style: unified quick+project list, in-place
 *            switching, "+ 新建" menu) PLUS an ADAPTIVE task tree shown only for
 *            project sessions — quick chats get no meaningless task column.
 *   Center — Execution stream (AI work in progress + chat input)
 *   Right  — Active skills + memory increments (transparency surface)
 */
export function WorkspacePage({
  sessionId,
  onNewConversation,
  onOpenSettings,
  onOpenUsage,
  onOpenSession,
  initialTaskLogId,
}: WorkspacePageProps) {
  const {
    activeSession, draftSession, sessions,
    sendOrQueue, steerRun, cancelStream, removeFromQueue, setDraftProject, setDraftAnonymous,
    respondPermission, exitAnonymous, renameSession, loadOlderMessages,
  } = useChatStore();
  const activeDraft = draftSession?.id === sessionId ? draftSession : null;
  // Per-session chat state for the ACTIVE session. Background sessions keep
  // streaming into their own buckets; here we render the active one's slice.
  const {
    messages,
    streaming,
    queue,
    pendingPermission,
    hasOlderHistory,
    loadingOlderHistory,
    historyTruncated,
  } = useChatStore(activeRuntime);
  const isAnonymous = activeSession?.kind === "anonymous";
  const persistedRunActive = useTasksStore((state) => state.running[sessionId] ?? false);
  const [pendingInsert, setPendingInsert] = useState<string | undefined>(undefined);
  const [deliveryState, setDeliveryState] = useState<WorkspaceDeliveryState>({
    snapshot: null,
    unavailable: false,
  });
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    try {
      return localStorage.getItem("cf.workspace.sidebarCollapsed") === "1";
    } catch {
      return false;
    }
  });
  const [narrowViewport, setNarrowViewport] = useState(() =>
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(max-width: 720px)").matches,
  );
  const [narrowSidebarOpen, setNarrowSidebarOpen] = useState(false);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(max-width: 720px)");
    const syncViewport = () => setNarrowViewport(media.matches);
    syncViewport();
    media.addEventListener("change", syncViewport);
    return () => media.removeEventListener("change", syncViewport);
  }, []);
  useEffect(() => {
    if (narrowViewport) setNarrowSidebarOpen(false);
  }, [narrowViewport, sessionId]);
  const sidebarVisible = narrowViewport ? narrowSidebarOpen : !sidebarCollapsed;
  const toggleSidebar = () => {
    if (narrowViewport) {
      const opening = !narrowSidebarOpen;
      setNarrowSidebarOpen(opening);
      if (opening) setBrowserPaneCollapsed(true);
      return;
    }
    setSidebarCollapsed((collapsed) => {
      const next = !collapsed;
      try {
        localStorage.setItem("cf.workspace.sidebarCollapsed", next ? "1" : "0");
      } catch {
        // Storage is optional; the current workspace still responds immediately.
      }
      return next;
    });
  };
  // Steering covers both in-flight surfaces: a streaming chat turn and an
  // autonomous task run. Same keystroke, same meaning, same queue — which of
  // the two happens to be running is the framework's business, not the user's.
  const steerActive = activeDraft ? false : streaming || persistedRunActive;
  const guideNextStep = async (message: string) => {
    const trimmed = message.trim();
    if (!trimmed || activeDraft) return;
    await steerRun(trimmed);
  };
  // Double-click the session title (here or in the sidebar) to rename it inline.
  const [titleEditing, setTitleEditing] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const commitTitle = () => {
    const t = titleDraft.trim();
    setTitleEditing(false);
    if (activeSession && t && t !== activeSession.title) renameSession(activeSession.id, t);
  };
  // Leave title-edit mode if the workspace switches to another session, so a
  // pending edit can't blur-commit onto the wrong session.
  useEffect(() => {
    setTitleEditing(false);
    setRequestedAuxiliaryPane(initialTaskLogId ? "tasks" : null);
    setGitPanel(null);
    setEvidenceId(null);
    setDeliveryState({ snapshot: null, unavailable: false });
  }, [initialTaskLogId, sessionId]);
  // Every secondary surface is arbitrated here. A single union prevents Git,
  // tasks, delivery, evidence, documents and the managed browser from stacking
  // independent drawers over the conversation.
  const [gitPanel, setGitPanel] = useState<"changes" | "history" | "remote" | null>(null);
  const [requestedAuxiliaryPane, setRequestedAuxiliaryPane] =
    useState<AuxiliaryPaneKind | null>(null);
  const [evidenceId, setEvidenceId] = useState<string | null>(null);
  const auxiliaryOpenerRef = useRef<HTMLElement | null>(null);
  const auxiliaryRestoreButtonRef = useRef<HTMLButtonElement>(null);
  const auxiliaryPaneRef = useRef<HTMLElement>(null);
  const workspaceMainRef = useRef<HTMLElement>(null);
  const [auxiliaryFocusRequest, setAuxiliaryFocusRequest] = useState(0);
  const handledAuxiliaryFocusRequestRef = useRef(0);
  const [viewportWidth, setViewportWidth] = useState(() =>
    typeof window === "undefined" ? 1440 : window.innerWidth,
  );
  const [auxiliaryPaneWidth, setAuxiliaryPaneWidth] = useState(() => {
    try {
      const persisted = Number(localStorage.getItem(AUXILIARY_PANE_STORAGE_KEY));
      return Number.isFinite(persisted) && persisted > 0
        ? clampAuxiliaryPaneWidth(persisted)
        : AUXILIARY_PANE_DEFAULT_WIDTH;
    } catch {
      return AUXILIARY_PANE_DEFAULT_WIDTH;
    }
  });
  const gitBranch = useGitStore((s) => s.status?.branch ?? "");
  const activeCwd = activeSession?.cwd ?? activeDraft?.cwd ?? null;
  const draftProjects = useMemo(() => recentProjects(sessions ?? []), [sessions]);
  const projectTasks = useTasksStore((state) => state.tasks[sessionId]);
  const projectTasksLoading = Boolean(useTasksStore((state) => state.loading?.[sessionId]));
  const projectTasksError = useTasksStore((state) => state.error?.[sessionId] ?? null);
  const sessionTasks = projectTasks ?? [];
  const externalJobs = useMemo<ExternalJobState[]>(
    () =>
      sessionTasks.map((task) => ({
        id: task.id,
        status: task.status,
        startedAt: task.started_at ? Date.parse(task.started_at) : null,
        completedAt: task.completed_at ? Date.parse(task.completed_at) : null,
      })),
    [sessionTasks],
  );
  const [turnTimingProfile, setTurnTimingProfile] =
    useState<TurnTimingProfile | null>(null);
  const projectTaskCount = sessionTasks.length;
  const taskRunningCount = sessionTasks.filter((task) => task.status === "running").length;
  const taskPendingCount = sessionTasks.filter((task) => task.status === "pending").length;
  const failedTasks = sessionTasks.filter((task) => task.status === "failed");
  const taskFailedCount = failedTasks.length;
  const blockedTasks = failedTasks.filter((task) => task.failure_attribution?.repairable === false);
  const coreInputTasks = blockedTasks.filter((task) =>
    ["model-provider", "permission"].includes(task.failure_attribution?.kind ?? ""),
  );
  const taskBlockedCount = coreInputTasks.length;
  const taskProviderBlockedCount = coreInputTasks.filter((task) => task.failure_attribution?.kind === "model-provider").length;
  const taskActivityVisible = taskPendingCount + taskRunningCount + taskFailedCount > 0;
  const taskActivitySummary = useMemo(() => {
    if (taskBlockedCount > 0) {
      const title = taskProviderBlockedCount === taskBlockedCount
        ? "模型配置需要处理"
        : "后台任务需要一项授权";
      const label = taskProviderBlockedCount === taskBlockedCount
        ? `${taskBlockedCount} 个后台任务需要修复模型配置`
        : `${taskBlockedCount} 个后台任务等待必要授权`;
      return { count: taskBlockedCount, label, title, tone: "danger" as const, kind: "blocked" as const };
    }
    if (taskFailedCount > 0) {
      return { count: taskFailedCount, label: `系统正在恢复 ${taskFailedCount} 个后台任务`, title: `系统正在恢复 ${taskFailedCount} 个后台任务`, tone: "warning" as const, kind: "failed" as const };
    }
    if (taskRunningCount > 0) {
      return { count: taskRunningCount, label: `${taskRunningCount} 个后台任务正在运行`, title: `${taskRunningCount} 个后台任务正在运行`, tone: "progress" as const, kind: "running" as const };
    }
    if (taskPendingCount > 0) {
      return { count: taskPendingCount, label: `${taskPendingCount} 个后台任务等待调度`, title: `${taskPendingCount} 个后台任务等待调度`, tone: "neutral" as const, kind: "pending" as const };
    }
    return null;
  }, [taskBlockedCount, taskFailedCount, taskPendingCount, taskProviderBlockedCount, taskRunningCount]);
  const taskActivityButtonRef = useRef<HTMLButtonElement>(null);
  const closeTaskActivity = useCallback(() => {
    setRequestedAuxiliaryPane(null);
    setBrowserPaneCollapsed(true);
    requestAnimationFrame(() => {
      if (taskActivityButtonRef.current?.isConnected) taskActivityButtonRef.current.focus();
      else if (auxiliaryOpenerRef.current?.isConnected) auxiliaryOpenerRef.current.focus();
      else if (auxiliaryRestoreButtonRef.current?.isConnected) auxiliaryRestoreButtonRef.current.focus();
      else workspaceMainRef.current?.focus();
    });
  }, []);
  const [browserSessions, setBrowserSessions] = useState<WorkspaceBrowserSession[]>([]);
  const [browserLoadError, setBrowserLoadError] = useState<string | null>(null);
  const [browserRefreshRequest, setBrowserRefreshRequest] = useState(0);
  const [browserPaneCollapsed, setBrowserPaneCollapsed] = useState(false);
  const [documentTabs, setDocumentTabs] = useState<DocumentTab[]>([]);
  const [activeRightTab, setActiveRightTab] = useState<string | null>(null);
  const loadProjectTasks = useTasksStore((state) => state.loadTasks);
  const subscribeProjectTasks = useTasksStore((state) => state.subscribe);
  const isProjectSession = Boolean(
    activeSession && activeSession.kind !== "quick" && activeSession.kind !== "anonymous",
  );
  const activeBrowserSessions = useMemo(
    () =>
      (Array.isArray(browserSessions) ? browserSessions : []).filter(
        (session) =>
          !session.expired &&
          session.owner_session_id === sessionId &&
          session.status !== "closed",
      ),
    [browserSessions, sessionId],
  );
  const selectedRightTab = useMemo(() => {
    const tabIds = new Set([
      ...activeBrowserSessions.map((session) => `browser:${session.session_id}`),
      ...documentTabs.map((tab) => tab.id),
    ]);
    if (activeRightTab && tabIds.has(activeRightTab)) return activeRightTab;
    return activeBrowserSessions[0]
      ? `browser:${activeBrowserSessions[0].session_id}`
      : documentTabs[0]?.id ?? null;
  }, [activeBrowserSessions, activeRightTab, documentTabs]);
  const passiveAuxiliaryPane: AuxiliaryPaneKind | null = selectedRightTab?.startsWith("document:")
    ? "document"
    : selectedRightTab?.startsWith("browser:")
      ? "browser"
      : null;
  const auxiliaryPaneKind = requestedAuxiliaryPane ?? passiveAuxiliaryPane;
  const auxiliaryPaneHasContent = auxiliaryPaneKind !== "tasks"
    ? auxiliaryPaneKind !== "evidence" || messages.some((message) => message.id === evidenceId)
    : projectTasks === undefined || projectTaskCount > 0;
  const auxiliaryPaneOpen = Boolean(auxiliaryPaneKind) && !browserPaneCollapsed && auxiliaryPaneHasContent;
  const taskActivityOpen = auxiliaryPaneOpen && auxiliaryPaneKind === "tasks";
  const auxiliaryLayout = auxiliaryLayoutForWidth(viewportWidth);
  const selectedBrowserSessionId = selectedRightTab?.startsWith("browser:")
    ? selectedRightTab.slice("browser:".length)
    : null;
  const auxiliaryPaneMaxWidth = Math.min(
    AUXILIARY_PANE_MAX_WIDTH,
    Math.max(
      AUXILIARY_PANE_MIN_WIDTH,
      viewportWidth - (sidebarVisible ? WORKSPACE_SIDEBAR_WIDTH : 0) - WORKSPACE_READING_MIN_WIDTH,
    ),
  );

  const openAuxiliaryPane = useCallback((kind: AuxiliaryPaneKind, opener?: HTMLElement | null) => {
    auxiliaryOpenerRef.current = opener ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    setAuxiliaryFocusRequest((request) => request + 1);
    setNarrowSidebarOpen(false);
    setRequestedAuxiliaryPane(kind);
    setBrowserPaneCollapsed(false);
  }, []);

  const collapseAuxiliaryPane = useCallback(() => {
    setBrowserPaneCollapsed(true);
    requestAnimationFrame(() => {
      const opener = auxiliaryOpenerRef.current;
      if (opener?.isConnected) opener.focus();
      else if (auxiliaryRestoreButtonRef.current) auxiliaryRestoreButtonRef.current.focus();
      else workspaceMainRef.current?.focus();
    });
  }, []);

  const embeddedBrowserEscapeStateRef = useRef({
    auxiliaryLayout,
    auxiliaryPaneKind,
    auxiliaryPaneOpen,
    pendingPermission: Boolean(pendingPermission),
    selectedBrowserSessionId,
  });
  embeddedBrowserEscapeStateRef.current = {
    auxiliaryLayout,
    auxiliaryPaneKind,
    auxiliaryPaneOpen,
    pendingPermission: Boolean(pendingPermission),
    selectedBrowserSessionId,
  };

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onEmbeddedBrowserEscape(({ session_id }) => {
      if (disposed) return;
      const state = embeddedBrowserEscapeStateRef.current;
      if (
        state.auxiliaryLayout === "dock" ||
        state.auxiliaryPaneKind !== "browser" ||
        !state.auxiliaryPaneOpen ||
        state.pendingPermission ||
        state.selectedBrowserSessionId !== session_id
      ) return;
      collapseAuxiliaryPane();
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // Browser preview remains usable through its explicit collapse button
        // when the native event bus is unavailable (for example, web-only dev).
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [collapseAuxiliaryPane]);

  const restoreAuxiliaryPane = useCallback(() => {
    auxiliaryOpenerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setAuxiliaryFocusRequest((request) => request + 1);
    setNarrowSidebarOpen(false);
    setBrowserPaneCollapsed(false);
  }, []);

  useEffect(() => {
    if (!auxiliaryPaneKind || auxiliaryPaneHasContent) return;
    setRequestedAuxiliaryPane(null);
    setBrowserPaneCollapsed(true);
    requestAnimationFrame(() => {
      if (auxiliaryOpenerRef.current?.isConnected) auxiliaryOpenerRef.current.focus();
      else workspaceMainRef.current?.focus();
    });
  }, [auxiliaryPaneHasContent, auxiliaryPaneKind]);

  useEffect(() => {
    const onResize = () => setViewportWidth(window.innerWidth);
    onResize();
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    setBrowserPaneCollapsed(false);
    setDocumentTabs([]);
    setActiveRightTab(null);
  }, [sessionId]);

  useEffect(() => {
    let cancelled = false;
    const refreshBrowserSessions = async () => {
      try {
        const sessions = await invoke<WorkspaceBrowserSession[]>("list_browser_sessions");
        if (!cancelled) {
          setBrowserSessions(Array.isArray(sessions) ? sessions : []);
          setBrowserLoadError(null);
        }
      } catch (error) {
        if (!cancelled) setBrowserLoadError(String(error));
      }
    };
    void refreshBrowserSessions();
    const timer = window.setInterval(() => {
      void refreshBrowserSessions();
    }, 2500);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [browserRefreshRequest, sessionId]);

  const closeBrowserPaneSession = async (browserSessionId: string) => {
    await invoke("close_browser_session", { sessionId: browserSessionId });
    setBrowserSessions((sessions) =>
      sessions.filter((session) => session.session_id !== browserSessionId),
    );
    requestAnimationFrame(() => {
      const target = auxiliaryPaneRef.current?.querySelector<HTMLElement>(
        '[role="tab"][tabindex="0"], [data-auxiliary-initial-focus]',
      );
      if (target) target.focus();
      else if (auxiliaryOpenerRef.current?.isConnected) auxiliaryOpenerRef.current.focus();
      else workspaceMainRef.current?.focus();
    });
  };

  useEffect(() => {
    let cancelled = false;
    if (!activeCwd || activeDraft) {
      setTurnTimingProfile(null);
      return () => {
        cancelled = true;
      };
    }
    void Promise.resolve(
      invoke<TurnTimingProfile>("get_turn_timing_profile", { cwd: activeCwd }),
    )
      .then((profile) => {
        if (!cancelled) setTurnTimingProfile(profile);
      })
      .catch(() => {
        if (!cancelled) setTurnTimingProfile(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activeCwd, activeDraft, messages.length]);

  // Subscribe as soon as a project Session opens. `delegate_tasks` may create
  // the first task from the chat agent, so waiting for a task panel to mount
  // would miss the event that makes that panel visible.
  useEffect(() => {
    if (!isProjectSession) return;
    void loadProjectTasks(sessionId);
    let unsubscribe: (() => void) | undefined;
    void subscribeProjectTasks(sessionId).then((stop) => { unsubscribe = stop; });
    return () => { unsubscribe?.(); };
  }, [isProjectSession, loadProjectTasks, sessionId, subscribeProjectTasks]);

  useEffect(() => {
    if (!auxiliaryPaneOpen || pendingPermission) return;
    const dialog = auxiliaryPaneRef.current;
    if (auxiliaryFocusRequest > handledAuxiliaryFocusRequestRef.current) {
      handledAuxiliaryFocusRequestRef.current = auxiliaryFocusRequest;
      requestAnimationFrame(() => {
        const target = dialog?.querySelector<HTMLElement>("[data-auxiliary-initial-focus], [data-dialog-initial-focus]") ?? dialog;
        target?.focus();
      });
    }
    if (auxiliaryLayout === "dock") return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        collapseAuxiliaryPane();
        return;
      }
      if (event.key !== "Tab" || !dialog || auxiliaryLayout !== "overlay") return;
      const focusable = Array.from(
        dialog.querySelectorAll<HTMLElement>(
          'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      ).filter((element) => !element.hasAttribute("hidden"));
      if (focusable.length === 0) {
        event.preventDefault();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [auxiliaryFocusRequest, auxiliaryLayout, auxiliaryPaneKind, auxiliaryPaneOpen, collapseAuxiliaryPane, gitPanel, pendingPermission]);

  useEffect(() => {
    try {
      localStorage.setItem(AUXILIARY_PANE_STORAGE_KEY, String(auxiliaryPaneWidth));
    } catch {
      // Persistence is optional; resize still works for the current session.
    }
  }, [auxiliaryPaneWidth]);

  return (
    <div className="h-full flex flex-col bg-surface-0">

      {/* ── Header ────────────────────────────────────────────────────────── */}
      <header aria-label="会话工具栏" className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b border-border/80 bg-surface-1/95 px-3 py-1.5">
        {!sidebarVisible && (
          <button
            type="button"
            onClick={toggleSidebar}
            className="flex h-11 shrink-0 items-center gap-1.5 rounded-lg px-2 text-[13px] font-medium text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:h-9"
            title="展开会话侧栏"
            aria-label="展开会话侧栏"
            aria-expanded={false}
            aria-controls="workspace-session-sidebar"
          >
            <MessageSquare size={14} aria-hidden="true" />
            <span>会话</span>
          </button>
        )}
        <div className="flex-1 min-w-0">
          <div className="flex truncate text-[13px] font-semibold text-gray-200 items-center gap-2">
            {titleEditing && activeSession ? (
              <input
                autoFocus
                value={titleDraft}
                onChange={(e) => setTitleDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitTitle();
                  if (e.key === "Escape") setTitleEditing(false);
                }}
                onBlur={commitTitle}
                className="min-w-0 flex-1 rounded-md border border-accent/50 bg-surface-2 px-1.5 py-0.5 text-[13px] text-gray-100 outline-none"
              />
            ) : (
              <span
                className="truncate"
                title={isAnonymous ? undefined : "双击重命名"}
                onDoubleClick={() => {
                  if (!isAnonymous && activeSession) {
                    setTitleDraft(activeSession.title || "");
                    setTitleEditing(true);
                  }
                }}
              >
                {activeSession?.title || "新会话"}
              </span>
            )}
            {activeDraft ? (
              <span
                className="rounded-md bg-status-progress-soft px-1.5 py-0.5 text-[11px] font-normal text-status-progress"
                title="尚未创建记录；发送首条消息后生成"
              >
                草稿
              </span>
            ) : activeSession?.kind === "quick" ? (
              <span
                className="rounded-md bg-surface-3 px-1.5 py-0.5 text-[11px] font-normal text-gray-400"
                title="没有绑定项目的独立任务"
              >
                独立任务
              </span>
            ) : null}
            {isAnonymous && (
              <span
                className="inline-flex items-center gap-1 rounded-md bg-status-warning-soft px-1.5 py-0.5 text-[11px] font-normal text-status-warning"
                title="匿名会话：不落库、不计费、不进记忆/画像。离开即丢弃。"
              >
                <EyeOff size={9} />
                匿名
              </span>
            )}
          </div>
          <div className="hidden truncate font-mono text-[11px] text-gray-600 lg:block">
            {isAnonymous
              ? "无痕会话 · 不落库 · 不计费 · 不学习"
              : activeDraft
                ? (activeDraft.cwd ?? "发送首条消息后创建会话")
                : activeSession?.cwd}
          </div>
        </div>
        {isAnonymous && (
          <button
            onClick={() => {
              exitAnonymous();
              onNewConversation(null);
            }}
            className="flex min-h-11 items-center gap-1 rounded-lg border border-status-warning/30 bg-status-warning-soft px-2 text-[13px] text-status-warning transition-colors hover:brightness-95 lg:min-h-9"
            title="退出匿名会话并丢弃其历史"
          >
            <EyeOff size={12} />
            退出匿名
          </button>
        )}
        <div className="flex items-center gap-1.5">
          <GitStatusBar
            cwd={activeCwd}
            detailsId={auxiliaryPaneOpen && auxiliaryPaneKind === "git"
              ? "workspace-auxiliary-pane"
              : undefined}
            detailsOpen={auxiliaryPaneOpen && auxiliaryPaneKind === "git"}
            onOpenChanges={() => {
              setGitPanel("changes");
              openAuxiliaryPane("git");
            }}
          />
          {!activeDraft && (
            <WorkspaceDeliveryStatus
              cwd={activeCwd}
              sessionId={sessionId}
              currentBranch={gitBranch}
              messages={messages}
              detailsOpen={auxiliaryPaneOpen && auxiliaryPaneKind === "delivery"}
              detailsId={auxiliaryPaneOpen && auxiliaryPaneKind === "delivery"
                ? "workspace-auxiliary-pane"
                : undefined}
              onOpenDetails={() => openAuxiliaryPane("delivery")}
              onDeliveryStateChange={setDeliveryState}
            />
          )}
        </div>
        {isProjectSession && taskActivityVisible && taskActivitySummary && (
          <button
            ref={taskActivityButtonRef}
            type="button"
            onClick={() => openAuxiliaryPane("tasks", taskActivityButtonRef.current)}
            aria-label={`打开任务活动：${taskActivitySummary.label}`}
            aria-expanded={taskActivityOpen}
            aria-controls={taskActivityOpen ? "workspace-auxiliary-pane" : undefined}
            className={`inline-flex h-11 min-w-11 items-center justify-center gap-1 rounded-lg px-2 text-[13px] font-medium transition-colors lg:h-9 lg:min-w-9 ${
              taskActivitySummary.tone === "danger"
                ? "bg-status-danger-soft text-status-danger hover:brightness-95"
                : taskActivitySummary.tone === "warning"
                  ? "bg-status-warning-soft text-status-warning hover:brightness-95"
                : taskActivitySummary.tone === "progress"
                  ? "bg-status-progress-soft text-status-progress hover:brightness-95"
                  : "text-gray-500 hover:bg-surface-3 hover:text-gray-300"
            }`}
            title={taskActivitySummary.title}
          >
            {taskActivitySummary.kind === "running" ? (
              <Loader2 size={12} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
            ) : taskActivitySummary.kind === "pending" ? (
              <Circle size={11} aria-hidden="true" />
            ) : taskActivitySummary.kind === "failed" ? (
              <AlertTriangle size={12} aria-hidden="true" />
            ) : (
              <XCircle size={12} aria-hidden="true" />
            )}
            <span>{taskActivitySummary.count}</span>
          </button>
        )}
        {browserPaneCollapsed && auxiliaryPaneKind && (
          <button
            ref={auxiliaryRestoreButtonRef}
            type="button"
            onClick={restoreAuxiliaryPane}
            aria-label={`恢复辅助工作区：${auxiliaryPaneLabel(auxiliaryPaneKind)}`}
            className="flex h-11 w-11 items-center justify-center rounded-lg text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200 lg:h-9 lg:w-9"
            title={`恢复${auxiliaryPaneLabel(auxiliaryPaneKind)}`}
          >
            <PanelRightOpen size={14} aria-hidden="true" />
          </button>
        )}
        <button
          onClick={() => onOpenSettings()}
          className="flex h-11 w-11 items-center justify-center rounded-lg text-gray-600 transition-colors hover:bg-surface-3 hover:text-gray-300 lg:h-9 lg:w-9"
          title="设置"
          aria-label="设置"
        >
          <SettingsIcon size={14} />
        </button>
      </header>

      {/* ── Body: 3 columns ──────────────────────────────────────────────── */}
      <div className="relative flex min-h-0 flex-1">

        {/* ─── Left: the rail owns collapse; the workspace header only restores it. ─── */}
        {narrowViewport && sidebarVisible && (
          <div
            aria-hidden="true"
            onMouseDown={() => setNarrowSidebarOpen(false)}
            className="absolute inset-0 z-30 bg-black/30"
          />
        )}
        {sidebarVisible && (
          <aside
            id="workspace-session-sidebar"
            aria-label="会话列表"
            className={`flex min-h-0 flex-col border-r border-border/80 bg-surface-1 ${
              narrowViewport
                ? "absolute inset-y-0 left-0 z-40 w-[min(272px,88vw)] shadow-2xl"
                : "w-[272px] shrink-0"
            }`}
          >
            <SessionSidebar
              currentSessionId={sessionId}
              onOpenSession={(id) => {
                if (narrowViewport) setNarrowSidebarOpen(false);
                onOpenSession(id);
              }}
              onNewConversation={(cwd) => {
                if (narrowViewport) setNarrowSidebarOpen(false);
                onNewConversation(cwd);
              }}
              onCollapse={toggleSidebar}
              collapseLabel={narrowViewport ? "关闭会话侧栏" : "收起会话侧栏"}
            />
          </aside>
        )}

        {/* ─── Center: conversation remains the primary surface. ─────────── */}
        <main
          ref={workspaceMainRef}
          tabIndex={-1}
          aria-label="会话窗口"
          data-auxiliary-pane={auxiliaryPaneOpen ? "open" : "closed"}
          className="flex min-w-0 flex-1 flex-col bg-surface-2"
        >
          <MessageList
            messages={messages}
            streaming={streaming}
            cwd={activeCwd}
            conversationKey={activeSession?.id ?? activeDraft?.id ?? sessionId}
            hasOlderHistory={hasOlderHistory}
            loadingOlderHistory={loadingOlderHistory}
            historyTruncated={historyTruncated}
            onLoadOlder={loadOlderMessages}
            onUsePrompt={(text) => setPendingInsert(text)}
            onOpenUsage={onOpenUsage}
            onOpenSession={onOpenSession}
            onPickProject={activeDraft ? setDraftProject : undefined}
            onOpenDocument={(path) => {
              auxiliaryOpenerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
              setNarrowSidebarOpen(false);
              const id = `document:${path}`;
              setDocumentTabs((tabs) => tabs.some((tab) => tab.id === id)
                ? tabs
                : [...tabs, { id, path, title: path.replace(/\\/g, "/").split("/").pop() ?? path }]);
              setActiveRightTab(id);
              setRequestedAuxiliaryPane(null);
              setBrowserPaneCollapsed(false);
            }}
            onOpenEvidence={(id) => {
              setEvidenceId(id);
              openAuxiliaryPane("evidence");
            }}
            evidenceControlsId={
              auxiliaryPaneOpen && auxiliaryPaneKind === "evidence"
                ? "workspace-auxiliary-pane"
                : undefined
            }
            openEvidenceMessageId={
              auxiliaryPaneOpen && auxiliaryPaneKind === "evidence" ? evidenceId : null
            }
            timingProfile={turnTimingProfile}
            externalJobs={externalJobs}
          />
          <div data-testid="workspace-composer-shell" className="shrink-0 bg-surface-2 px-3 pb-3 pt-2">
            <div className="mx-auto w-full max-w-[880px] overflow-hidden rounded-2xl border border-border/80 bg-surface-2 shadow-lg">
              {queue.length > 0 && (
                <QueueBadge queue={queue} onRemove={removeFromQueue} />
              )}
              {/* A draft's two remaining choices — where it works, and whether it
                  leaves a trace — live inside the composer surface because they
                  stop being editable the moment the first message is sent. */}
              {activeDraft && (
                <DraftScopeBar
                  cwd={activeDraft.cwd}
                  anonymous={activeDraft.anonymous}
                  projects={draftProjects}
                  onPickProject={setDraftProject}
                  onToggleAnonymous={setDraftAnonymous}
                />
              )}
              <MessageInput
                key={activeSession?.id ?? activeDraft?.id ?? sessionId}
                initialHistory={messages.filter((m) => m.role === "user").map((m) => m.content)}
                onSend={(t) => void sendOrQueue(t)}
                onGuide={guideNextStep}
                onCancel={() => cancelStream()}
                streaming={streaming}
                guidanceActive={steerActive}
                disabled={!activeSession && !activeDraft}
                pendingInsert={pendingInsert}
                onInsertConsumed={() => setPendingInsert(undefined)}
                cwd={activeCwd}
              />
              <div
                role="group"
                aria-label="下一回合控制"
                className="flex min-h-10 flex-wrap items-center gap-1.5 border-t border-border/60 bg-surface-1/35 px-3 py-1.5"
              >
                <ModelPicker portal />
                {!activeDraft && !isAnonymous && <ReasoningEffortPicker />}
                {!activeDraft && !isAnonymous && <PermissionModePicker />}
                {!activeDraft && (
                  <div className="ml-auto">
                    <ContextUsageBar
                      sessionId={activeSession?.id}
                      onOpenUsage={onOpenUsage ? () => onOpenUsage() : undefined}
                    />
                  </div>
                )}
              </div>
            </div>
          </div>
        </main>

        {auxiliaryPaneOpen && auxiliaryPaneKind && (
          <>
          {auxiliaryLayout !== "dock" && (
            <div
              aria-hidden="true"
              className={`absolute inset-0 z-30 ${auxiliaryLayout === "overlay" ? "bg-black/40" : "bg-black/15"}`}
              onMouseDown={collapseAuxiliaryPane}
            />
          )}
          <WorkspaceAuxiliaryPane
            paneRef={auxiliaryPaneRef}
            kind={auxiliaryPaneKind}
            layout={auxiliaryLayout}
            width={auxiliaryPaneWidth}
            maxWidth={auxiliaryPaneMaxWidth}
            sessions={activeBrowserSessions}
            documents={documentTabs}
            activeTab={selectedRightTab}
            loadError={browserLoadError}
            suspendNativeBrowser={Boolean(pendingPermission)}
            onRetryBrowserSessions={() => setBrowserRefreshRequest((request) => request + 1)}
            cwd={activeCwd}
            onSelectTab={(id) => {
              setActiveRightTab(id);
              setRequestedAuxiliaryPane(null);
            }}
            onCloseDocument={(id) => {
              setDocumentTabs((tabs) => tabs.filter((tab) => tab.id !== id));
              setActiveRightTab((current) => current === id ? null : current);
              requestAnimationFrame(() => {
                const target = auxiliaryPaneRef.current?.querySelector<HTMLElement>(
                  '[role="tab"][tabindex="0"], [data-auxiliary-initial-focus]',
                );
                if (target) target.focus();
                else if (auxiliaryOpenerRef.current?.isConnected) auxiliaryOpenerRef.current.focus();
                else workspaceMainRef.current?.focus();
              });
            }}
            onCollapse={collapseAuxiliaryPane}
            onCloseSession={(browserSessionId) => void closeBrowserPaneSession(browserSessionId)}
            onResize={setAuxiliaryPaneWidth}
            content={
              auxiliaryPaneKind === "git" ? (
                gitPanel === "history" ? (
                  <GitHistoryPanel embedded onClose={() => {
                    setGitPanel("changes");
                    setAuxiliaryFocusRequest((request) => request + 1);
                  }} />
                ) : gitPanel === "remote" ? (
                  <RemoteGitPanel embedded currentBranch={gitBranch} onClose={() => {
                    setGitPanel("changes");
                    setAuxiliaryFocusRequest((request) => request + 1);
                  }} />
                ) : (
                  <GitChangesPanel
                    embedded
                    sessionId={activeDraft ? null : sessionId}
                    onOpenHistory={() => {
                      setGitPanel("history");
                      setAuxiliaryFocusRequest((request) => request + 1);
                    }}
                    onOpenRemote={() => {
                      setGitPanel("remote");
                      setAuxiliaryFocusRequest((request) => request + 1);
                    }}
                    onClose={collapseAuxiliaryPane}
                  />
                )
              ) : auxiliaryPaneKind === "tasks" ? (
                isProjectSession && projectTaskCount > 0 ? (
                  <TasksColumn
                    sessionId={sessionId}
                    highlightedTaskId={initialTaskLogId}
                    onOpenSettings={onOpenSettings}
                    onFocusPermission={() => {
                      collapseAuxiliaryPane();
                      requestAnimationFrame(() => {
                        document.getElementById("workspace-permission-mode")?.focus();
                      });
                    }}
                    onClose={closeTaskActivity}
                  />
                ) : projectTasksError ? (
                  <TaskActivityState
                    error={projectTasksError}
                    onRetry={() => void loadProjectTasks(sessionId)}
                    onClose={closeTaskActivity}
                  />
                ) : projectTasksLoading || projectTasks === undefined ? (
                  <TaskActivityState onClose={closeTaskActivity} />
                ) : (
                  null
                )
              ) : auxiliaryPaneKind === "delivery" ? (
                <WorkspaceDeliveryStatus
                  detailsOnly
                  cwd={activeCwd}
                  sessionId={sessionId}
                  currentBranch={gitBranch}
                  messages={messages}
                  onCloseDetails={collapseAuxiliaryPane}
                  deliveryState={deliveryState}
                />
              ) : auxiliaryPaneKind === "evidence" ? (
                <TurnEvidencePane evidenceId={evidenceId} messages={messages} onClose={collapseAuxiliaryPane} />
              ) : null
            }
          />
          </>
        )}

      </div>

      {/* ── Permission dialog overlay ───────────────────────────────────── */}
      {pendingPermission && (
        <PermissionDialog
          request={pendingPermission}
          trusted={(activeSession?.permission_mode ?? "standard") === "trusted"}
          onAllow={() => respondPermission(true)}
          onDeny={() => respondPermission(false)}
          onAllowFullAccess={() => respondPermission(true, { grantFullAccess: true })}
        />
      )}
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// EmbeddedBrowserPane
// ─────────────────────────────────────────────────────────────────────────────

function WorkspaceAuxiliaryPane({ paneRef, kind, layout, width, maxWidth, sessions, documents, activeTab, loadError, suspendNativeBrowser, cwd, content, onSelectTab, onCloseDocument, onCollapse, onCloseSession, onResize, onRetryBrowserSessions }: {
  paneRef: RefObject<HTMLElement>;
  kind: AuxiliaryPaneKind;
  layout: AuxiliaryLayout;
  width: number;
  maxWidth: number;
  sessions: WorkspaceBrowserSession[];
  documents: DocumentTab[];
  activeTab: string | null;
  loadError: string | null;
  suspendNativeBrowser: boolean;
  cwd?: string | null;
  content: ReactNode;
  onSelectTab: (id: string) => void;
  onCloseDocument: (id: string) => void;
  onCollapse: () => void;
  onCloseSession: (sessionId: string) => void;
  onResize: (width: number) => void;
  onRetryBrowserSessions: () => void;
}) {
  const tabRefs = useRef(new Map<string, HTMLButtonElement>());
  const tabs = [
    ...sessions.map((session) => ({ id: `browser:${session.session_id}`, label: sessionTitle(session), kind: "browser" as const })),
    ...documents.map((document) => ({ id: document.id, label: document.title, kind: "document" as const })),
  ];
  const selected = tabs.some((tab) => tab.id === activeTab) ? activeTab : tabs[0]?.id ?? null;
  const activeBrowser = sessions.find((session) => `browser:${session.session_id}` === selected) ?? null;
  const activeDocument = documents.find((tab) => tab.id === selected) ?? null;
  const tabbed = kind === "browser" || kind === "document";
  const resizable = tabbed || kind === "git";
  const effectiveWidth = resizable
    ? clampAuxiliaryPaneWidth(width, maxWidth)
    : AUXILIARY_STATUS_PANE_WIDTH;
  const activeTabDomId = selected ? `workspace-aux-tab-${selected.replace(/[^a-zA-Z0-9_-]/g, "-")}` : undefined;
  const panelId = "workspace-auxiliary-tabpanel";

  const selectAdjacentTab = (currentId: string, direction: -1 | 1) => {
    const currentIndex = tabs.findIndex((tab) => tab.id === currentId);
    if (currentIndex < 0 || tabs.length === 0) return;
    const next = tabs[(currentIndex + direction + tabs.length) % tabs.length];
    onSelectTab(next.id);
    requestAnimationFrame(() => tabRefs.current.get(next.id)?.focus());
  };

  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = width;
    const move = (pointerEvent: PointerEvent) => {
      onResize(clampAuxiliaryPaneWidth(startWidth + startX - pointerEvent.clientX, maxWidth));
    };
    const end = () => {
      document.removeEventListener("pointermove", move);
      document.removeEventListener("pointerup", end);
    };
    document.addEventListener("pointermove", move);
    document.addEventListener("pointerup", end);
  };

  const paneStyle = layout === "overlay"
    ? { width: "100%" }
    : { width: `${Math.min(effectiveWidth, viewportSafeWidth())}px`, maxWidth: "100%" };
  const paneClass = layout === "dock"
    ? "relative z-20 flex min-h-0 shrink-0 flex-col border-l border-border bg-surface-1"
    : "absolute inset-y-0 right-0 z-40 flex min-h-0 flex-col border-l border-border bg-surface-1 shadow-2xl";

  return (
    <aside
      ref={paneRef}
      id="workspace-auxiliary-pane"
      data-testid="workspace-auxiliary-pane"
      data-pane-kind={kind}
      data-layout={layout}
      aria-label={kind === "tasks" && layout !== "dock" ? "任务活动" : "辅助工作区"}
      role={layout === "dock" ? "complementary" : "dialog"}
      aria-modal={layout === "dock" ? undefined : layout === "overlay"}
      tabIndex={-1}
      className={paneClass}
      style={paneStyle}
    >
      {resizable && layout === "dock" && (
        <div
          role="separator"
          tabIndex={0}
          aria-label="调整辅助工作区宽度"
          aria-orientation="vertical"
          aria-valuemin={AUXILIARY_PANE_MIN_WIDTH}
          aria-valuemax={maxWidth}
          aria-valuenow={effectiveWidth}
          onPointerDown={beginResize}
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
              event.preventDefault();
              const delta = event.key === "ArrowLeft" ? 24 : -24;
              onResize(clampAuxiliaryPaneWidth(effectiveWidth + delta, maxWidth));
            }
          }}
          className="absolute inset-y-0 -left-1.5 z-10 w-3 cursor-col-resize outline-none after:absolute after:inset-y-0 after:left-1/2 after:w-px after:bg-border hover:after:bg-accent focus:after:bg-accent"
        />
      )}
      {tabbed && <div className="flex min-w-0 items-center gap-1 border-b border-border px-2 py-1.5">
        <div role="tablist" aria-label="右侧工作区标签" className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
          {tabs.map((tab) => <button
            key={tab.id}
            id={`workspace-aux-tab-${tab.id.replace(/[^a-zA-Z0-9_-]/g, "-")}`}
            ref={(element) => { if (element) tabRefs.current.set(tab.id, element); else tabRefs.current.delete(tab.id); }}
            type="button"
            role="tab"
            aria-selected={selected === tab.id}
            aria-controls={panelId}
            tabIndex={selected === tab.id ? 0 : -1}
            onClick={() => onSelectTab(tab.id)}
            onKeyDown={(event) => {
              if (event.key === "ArrowLeft") { event.preventDefault(); selectAdjacentTab(tab.id, -1); }
              if (event.key === "ArrowRight") { event.preventDefault(); selectAdjacentTab(tab.id, 1); }
            }}
            className={`flex min-h-11 min-w-11 max-w-44 shrink-0 items-center gap-1 rounded px-2 py-1 text-[11px] lg:min-h-9 lg:min-w-9 ${selected === tab.id ? "bg-surface-3 text-gray-200" : "text-gray-500 hover:bg-surface-2 hover:text-gray-300"}`}
          >
            {tab.kind === "browser" ? <Globe2 size={11} /> : <FileText size={11} />}
            <span className="truncate">{tab.label}</span>
          </button>)}
        </div>
        <button data-auxiliary-initial-focus type="button" onClick={onCollapse} aria-label="折叠辅助工作区" className="min-h-11 min-w-11 shrink-0 rounded px-2 text-[11px] text-gray-500 hover:bg-surface-3 hover:text-gray-200 lg:min-h-9 lg:min-w-9">折叠</button>
      </div>}
      {tabbed ? (
        <div id={panelId} role="tabpanel" aria-labelledby={activeTabDomId} className="flex min-h-0 flex-1 flex-col">
          {activeBrowser && loadError && (
            <div role="alert" className="flex items-center gap-2 border-b border-red-500/20 bg-red-500/10 px-3 py-2 text-[11px] text-red-700 dark:text-red-300">
              <span className="min-w-0 flex-1 break-words">浏览器状态读取失败：{loadError}</span>
              <button type="button" onClick={onRetryBrowserSessions} className="min-h-11 shrink-0 rounded-lg px-2 hover:bg-red-500/10 lg:min-h-9">重试</button>
            </div>
          )}
          {activeDocument ? (
            <DocumentPreview tab={activeDocument} cwd={cwd} onClose={() => onCloseDocument(activeDocument.id)} />
          ) : activeBrowser ? (
            <EmbeddedBrowserPane sessions={[activeBrowser]} width={effectiveWidth} suspended={suspendNativeBrowser} onCloseSession={onCloseSession} />
          ) : null}
        </div>
      ) : content}
    </aside>
  );
}

function viewportSafeWidth(): number {
  return typeof window === "undefined" ? AUXILIARY_PANE_MAX_WIDTH : Math.max(0, window.innerWidth);
}

function TaskActivityState({ error, onRetry, onClose }: {
  error?: string | null;
  onRetry?: () => void;
  onClose: () => void;
}) {
  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <header className="flex items-center gap-3 border-b border-border px-4 py-3">
        <PanelRightOpen size={15} className="text-gray-500" aria-hidden="true" />
        <h2 className="flex-1 text-sm font-semibold text-gray-200">任务活动</h2>
        <button
          data-auxiliary-initial-focus
          type="button"
          onClick={onClose}
          aria-label="关闭任务活动"
          className="flex h-11 w-11 items-center justify-center rounded-lg text-gray-500 hover:bg-surface-3 hover:text-gray-200 lg:h-9 lg:w-9"
        >
          <X size={14} aria-hidden="true" />
        </button>
      </header>
      {error ? (
        <div role="alert" className="m-4 rounded-lg border border-status-danger/25 bg-status-danger-soft p-4 text-xs text-status-danger">
          <p className="break-words">任务活动读取失败：{error}</p>
          <button
            type="button"
            onClick={onRetry}
            className="mt-3 min-h-11 min-w-11 rounded-lg border border-current/20 px-3 font-medium hover:brightness-95 lg:min-h-9 lg:min-w-9"
          >
            重试加载任务
          </button>
        </div>
      ) : (
        <div role="status" className="m-4 flex items-center justify-center gap-2 rounded-lg border border-dashed border-border px-3 py-8 text-center text-xs text-gray-500">
          <Loader2 size={14} className="animate-spin motion-reduce:animate-none" aria-hidden="true" />
          正在加载任务活动
        </div>
      )}
    </section>
  );
}

// EmbeddedBrowserPane remains the isolated native-webview renderer. Its header
// is supplied by RightWorkbenchPane so browser and document tabs share one rail.
function EmbeddedBrowserPane({ sessions, width, suspended, onCloseSession }: {
  sessions: WorkspaceBrowserSession[];
  width: number;
  suspended: boolean;
  onCloseSession: (sessionId: string) => void;
}) {
  const active = sessions[0];
  if (!active) return null;
  const host = sessionHost(active);
  const title = sessionTitle(active);
  const safeUrl = active.pane_url && /^https?:\/\//i.test(active.pane_url) ? active.pane_url : null;
  const viewportRef = useRef<HTMLDivElement>(null);
  const mountInFlightRef = useRef(false);
  const mountFailedRef = useRef(false);
  const visibilityFallbackRef = useRef(false);
  const suspendedRef = useRef(suspended);
  const [mountError, setMountError] = useState<string | null>(null);
  suspendedRef.current = suspended;

  const syncNativeWebView = useCallback(async () => {
    if (
      suspendedRef.current ||
      mountFailedRef.current ||
      mountInFlightRef.current ||
      !safeUrl ||
      !viewportRef.current
    ) return;
    const rect = viewportRef.current.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    mountInFlightRef.current = true;
    try {
      await invoke("embedded_browser_mount", {
        sessionId: active.session_id,
        url: safeUrl,
        bounds: { x: rect.left, y: rect.top, width: rect.width, height: rect.height },
      });
      setMountError(null);
    } catch (error) {
      mountFailedRef.current = true;
      setMountError(String(error));
    } finally {
      mountInFlightRef.current = false;
    }
  }, [active.session_id, safeUrl]);

  useEffect(() => {
    mountFailedRef.current = false;
    setMountError(null);
    void syncNativeWebView();
    if (!safeUrl) return;
    const onResize = () => { void syncNativeWebView(); };
    window.addEventListener("resize", onResize);
    const timer = window.setInterval(() => { void syncNativeWebView(); }, 1000);
    return () => {
      window.removeEventListener("resize", onResize);
      window.clearInterval(timer);
      void invoke("embedded_browser_unmount", { sessionId: active.session_id }).catch(() => {});
    };
  }, [active.session_id, safeUrl, syncNativeWebView]);

  useEffect(() => {
    if (!safeUrl) return;
    if (!suspended && visibilityFallbackRef.current) {
      visibilityFallbackRef.current = false;
      mountFailedRef.current = false;
      setMountError(null);
    }
    void invoke("embedded_browser_set_visible", {
      sessionId: active.session_id,
      visible: !suspended,
    }).catch((error) => {
      if (!suspended) {
        setMountError(`内置浏览器可见性恢复失败：${String(error)}`);
        return;
      }
      // A native child sits above the DOM compositor. If hiding it fails while
      // a permission modal owns the foreground, close only the child webview
      // (not the managed browser session) so it cannot intercept approval input.
      visibilityFallbackRef.current = true;
      setMountError(`权限确认前无法暂停内置浏览器：${String(error)}`);
      void invoke("embedded_browser_unmount", { sessionId: active.session_id }).catch(() => {});
    });
    if (!suspended) void syncNativeWebView();
  }, [active.session_id, safeUrl, suspended, syncNativeWebView]);

  return (
    <div
      aria-label="内置浏览器"
      data-browser-width={String(width)}
      className="flex min-h-0 flex-1 flex-col bg-surface-1"
    >
      <div className="flex items-center gap-2 border-b border-border px-3 py-2">
        <Globe2 size={14} className="shrink-0 text-accent" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs font-medium text-gray-200">{host}</div>
          <div className="truncate text-[11px] text-gray-500">{title}</div>
        </div>
        {sessions.length > 1 && <span className="rounded-full bg-accent/10 px-1.5 py-0.5 text-[11px] text-accent">{sessions.length}</span>}
        <button type="button" onClick={() => onCloseSession(active.session_id)} className="min-h-11 min-w-11 rounded bg-red-500/10 px-2 text-[11px] text-red-700 hover:bg-red-500/20 dark:text-red-300 lg:min-h-9 lg:min-w-9" aria-label="结束浏览器" title="结束当前会话的受管浏览器">结束</button>
      </div>
      <div className="flex min-h-0 flex-1 flex-col bg-surface-0">
        {safeUrl ? (
          <div
            ref={viewportRef}
            role="application"
            aria-label={`网页视图：${title}`}
            className="relative min-h-0 flex-1 overflow-hidden bg-white"
            data-codefactory-browser-session={active.session_id}
          >
            {mountError ? (
              <div role="alert" className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-white p-6 text-center text-xs text-red-700">
                <div>
                  <p className="font-medium">内置浏览器打开失败</p>
                  <p className="mt-1 max-w-sm break-words text-gray-600">{mountError}</p>
                </div>
                <button
                  type="button"
                  aria-label="重试内置浏览器"
                  onClick={() => {
                    mountFailedRef.current = false;
                    setMountError(null);
                    void syncNativeWebView();
                  }}
                  className="min-h-11 rounded-lg border border-red-200 px-3 text-xs text-red-700 hover:bg-red-50 lg:min-h-9"
                >
                  重试
                </button>
              </div>
            ) : (
              <div className="absolute inset-0 flex items-center justify-center bg-white text-xs text-gray-600">
                {suspended ? "权限确认期间已暂停网页交互" : `正在内置浏览器中打开 ${host}`}
              </div>
            )}
          </div>
        ) : (
          <div className="flex flex-1 items-center justify-center p-6 text-center text-xs text-gray-500">
            <div>
              <ExternalLink size={18} className="mx-auto mb-2 text-gray-600" aria-hidden="true" />
              <p className="font-medium text-gray-300">受管浏览器已连接</p>
              <p className="mt-1">等待页面地址；Agent 可继续读取和操作该会话。</p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function turnEvidenceArgs(args: string): { summary: string | null; full: string } {
  try {
    const parsed = JSON.parse(args) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return { summary: null, full: args };
    }
    const record = parsed as Record<string, unknown>;
    const summary = [record.path, record.command, record.cwd, record.url]
      .find((value): value is string => typeof value === "string" && value.trim().length > 0)
      ?? null;
    return { summary, full: JSON.stringify(record, null, 2) };
  } catch {
    return { summary: args.trim() || null, full: args };
  }
}

export function TurnEvidencePane({ evidenceId, messages, onClose }: {
  evidenceId: string | null;
  messages: UIMessage[];
  onClose: () => void;
}) {
  const message = messages.find((candidate) => candidate.id === evidenceId) ?? null;
  const calls = message?.turnToolCalls ?? message?.toolCalls ?? [];
  const totalCallCount = Math.max(calls.length, message?.turnToolCallCount ?? 0);
  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <header className="flex items-start gap-3 border-b border-border px-4 py-3">
        <CheckCircle2 size={15} className="mt-0.5 text-accent" aria-hidden="true" />
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold text-gray-100">回合证据</h2>
          <p className="mt-0.5 text-[11px] text-gray-600">本回合实际执行的工具、验证与失败边界。</p>
        </div>
        <button data-auxiliary-initial-focus type="button" onClick={onClose} aria-label="关闭回合证据" className="flex h-11 w-11 items-center justify-center rounded text-gray-500 hover:bg-surface-3 hover:text-gray-200 lg:h-9 lg:w-9"><X size={14} /></button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {!message ? (
          <p className="rounded-lg border border-dashed border-border px-3 py-8 text-center text-xs text-gray-600">该回合的证据仍保留在消息结果卡中。</p>
        ) : (
          <div className="space-y-3">
            {message.failureEvidence && <div className="rounded-lg border border-status-warning/25 bg-status-warning-soft p-3 text-xs leading-5 text-status-warning">{message.failureEvidence}</div>}
            {totalCallCount > calls.length && (
              <p role="status" className="rounded-lg border border-status-info/25 bg-status-info-soft px-3 py-2 text-xs text-status-info">
                仅显示最近 {calls.length}/{totalCallCount} 项操作
              </p>
            )}
            <ol className="space-y-1" aria-label="回合操作证据">
              {calls.map((call) => {
                const input = turnEvidenceArgs(call.args);
                return (
                  <li key={call.id} className="flex items-start gap-2 rounded-lg border border-border/60 bg-surface-2 px-3 py-2 text-xs">
                    {call.status === "done" && !call.isError ? <CheckCircle2 size={13} aria-hidden="true" className="mt-0.5 shrink-0 text-status-success" /> : <AlertTriangle size={13} aria-hidden="true" className="mt-0.5 shrink-0 text-status-warning" />}
                    <div className="min-w-0 flex-1 space-y-1.5">
                      <div className="font-mono text-gray-300">{call.name}</div>
                      {input.summary && <code className="block break-all text-[11px] text-gray-500">{input.summary}</code>}
                      {input.full && (
                        <details>
                          <summary className="cursor-pointer text-[11px] text-gray-500 hover:text-gray-300">完整输入</summary>
                          <pre className="mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-words rounded bg-surface-0 p-2 text-[11px] text-gray-500">{input.full}</pre>
                        </details>
                      )}
                      {call.result && (
                        <details open={Boolean(call.isError)}>
                          <summary className="cursor-pointer text-[11px] text-gray-500 hover:text-gray-300">完整输出</summary>
                          <pre className="mt-1 max-h-80 overflow-auto whitespace-pre-wrap break-words rounded bg-surface-0 p-2 text-[11px] text-gray-500">{call.result}</pre>
                        </details>
                      )}
                    </div>
                  </li>
                );
              })}
            </ol>
            {calls.length === 0 && !message.failureEvidence && <p className="text-xs text-gray-600">本回合没有工具或失败证据。</p>}
          </div>
        )}
      </div>
    </section>
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// TasksColumn
// ─────────────────────────────────────────────────────────────────────────────

// Task decomposition is internal to the conversation; this panel only renders
// the execution detail after the agent has delegated work.
function TasksColumn({ sessionId, highlightedTaskId, onOpenSettings, onFocusPermission, onClose }: {
  sessionId: string;
  highlightedTaskId?: string | null;
  onOpenSettings: (tab: "endpoints") => void;
  onFocusPermission: () => void;
  onClose: () => void;
}) {
  const { tasks, running, cancel } = useTasksStore();
  const sessionTasks: TaskRun[] = tasks[sessionId] ?? [];
  const isRunning = running[sessionId] ?? false;
  const pendingCount = sessionTasks.filter((task) => task.status === "pending").length;
  const runningCount = sessionTasks.filter((task) => task.status === "running").length;
  const completedCount = sessionTasks.filter((task) => task.status === "completed").length;
  const failedTasks = sessionTasks.filter((task) => task.status === "failed");
  const blockedTasks = failedTasks.filter((task) => task.failure_attribution?.repairable === false);
  const providerBlockedTasks = blockedTasks.filter((task) => task.failure_attribution?.kind === "model-provider");
  const permissionBlockedTasks = blockedTasks.filter((task) => task.failure_attribution?.kind === "permission");
  const systemRecoveryTasks = failedTasks.filter((task) => !["model-provider", "permission"].includes(task.failure_attribution?.kind ?? "unknown"));
  const [startError, setStartError] = useState<string | null>(null);

  const handleCancel = async () => {
    try { await cancel(sessionId); } catch (error) { setStartError(String(error)); }
  };
  // Task decomposition is an implementation detail of the conversation.
  if (sessionTasks.length === 0) return null;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-12 shrink-0 items-center justify-between border-b border-border px-4">
        <div className="min-w-0">
          <h2 className="text-sm font-medium text-gray-200">任务活动</h2>
          <p className="text-[11px] text-gray-600">后台步骤、验收结果与恢复操作</p>
        </div>
        <button data-dialog-initial-focus type="button" onClick={onClose} aria-label="关闭任务活动" className="flex h-11 w-11 items-center justify-center rounded-lg text-gray-500 hover:bg-surface-3 hover:text-gray-200 lg:h-9 lg:w-9">
          <X size={15} />
        </button>
      </div>
      <div className="flex shrink-0 flex-col gap-2 border-b border-border px-4 py-3">
        <div className="flex flex-wrap items-center gap-2 text-[12px] text-gray-600">
          <span>已完成 {completedCount}</span><span>等待调度 {pendingCount}</span>
          {runningCount > 0 && <span className="text-status-progress">执行中 {runningCount}</span>}
          {failedTasks.length > 0 && <span className="text-status-warning">系统恢复 {failedTasks.length}</span>}
          {providerBlockedTasks.length > 0 && <span className="text-status-danger">模型配置 {providerBlockedTasks.length}</span>}
          {permissionBlockedTasks.length > 0 && <span className="text-status-danger">权限配置 {permissionBlockedTasks.length}</span>}
          {systemRecoveryTasks.length > 0 && <span className="text-status-warning">正在诊断 {systemRecoveryTasks.length}</span>}
        </div>
        {!isRunning && pendingCount > 0 && (
          <p className={`text-[12px] leading-5 ${failedTasks.length > 0 ? "text-status-warning" : "text-gray-500"}`}>
            {failedTasks.length > 0
              ? `系统正在处理失败项，并会自动续接剩余 ${pendingCount} 项。`
              : `已委派，还有 ${pendingCount} 项等待后台调度。`}
          </p>
        )}
        <div className="flex flex-wrap items-center gap-1">
          {isRunning ? (
            <button onClick={() => void handleCancel()} className="flex min-h-11 items-center gap-1.5 rounded-lg bg-status-danger-soft px-2.5 text-[13px] text-status-danger hover:brightness-95 lg:min-h-9"><Square size={11} />停止</button>
          ) : pendingCount > 0 && failedTasks.length === 0 ? (
            <span className="text-[12px] leading-5 text-gray-500">任务已委派，系统会持续调度并自动诊断恢复，无需手动重试。</span>
          ) : null}
          {!isRunning && providerBlockedTasks.length > 0 && (
            <button onClick={() => onOpenSettings("endpoints")} className="min-h-8 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress hover:brightness-95">打开模型设置</button>
          )}
          {!isRunning && permissionBlockedTasks.length > 0 && (
            <button onClick={onFocusPermission} className="min-h-8 rounded-lg bg-status-progress-soft px-2.5 text-[13px] text-status-progress hover:brightness-95">调整会话权限</button>
          )}
          {!isRunning && systemRecoveryTasks.length > 0 && (
            <span role="status" className="text-[12px] leading-5 text-status-warning">
              CodeFactory 正在诊断并自动续接，无需手动处理。
            </span>
          )}
        </div>
      </div>
      {startError && <div className="border-b border-status-danger/20 bg-status-danger-soft px-4 py-2 text-[12px] text-status-danger">{startError}</div>}
      <div className="flex-1 overflow-y-auto p-3"><ul className="space-y-1">{buildTaskTree(sessionTasks).map(({ task, depth }) => <TaskRow key={task.id} task={task} depth={depth} highlighted={task.id === highlightedTaskId} />)}</ul></div>
    </div>
  );
}

function buildTaskTree(tasks: TaskRun[]): { task: TaskRun; depth: number }[] {
  const byParent = new Map<string | null, TaskRun[]>();
  for (const t of tasks) {
    const k = t.parent_task_id;
    if (!byParent.has(k)) byParent.set(k, []);
    byParent.get(k)!.push(t);
  }
  const out: { task: TaskRun; depth: number }[] = [];
  const walk = (parentId: string | null, depth: number) => {
    for (const t of byParent.get(parentId) ?? []) {
      out.push({ task: t, depth });
      walk(t.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

function TaskRow({ task, depth, highlighted = false }: { task: TaskRun; depth: number; highlighted?: boolean }) {
  const Icon = statusIcon(task.status);
  // Surface acceptance-criteria verification right here in the task tree — the
  // "did it actually pass?" proof that previously only lived in evidence packs.
  const verif = parseVerification(task.verification_results);
  const summary = verificationSummary(task.verification_results);
  const [verifOpen, setVerifOpen] = useState(false);
  return (
    <li
      id={`task-log-${task.id}`}
      aria-current={highlighted ? "true" : undefined}
      className={`rounded-lg transition-colors ${highlighted ? "bg-status-progress-soft ring-1 ring-status-progress/30" : "hover:bg-surface-3"}`}
      style={{ paddingLeft: `${0.375 + depth * 0.875}rem` }}
    >
      <div className="group flex items-start gap-2 px-1.5 py-1">
        <Icon
          size={13}
          className={`mt-1 shrink-0 ${statusColor(task.status)} ${
            task.status === "running" ? "animate-spin motion-reduce:animate-none" : ""
          }`}
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-start gap-1.5">
            <span className="block flex-1 text-[13px] leading-5 text-gray-300 line-clamp-2">
              {task.title}
            </span>
            {summary && (
              <button
                onClick={() => setVerifOpen((v) => !v)}
                title={`验收验证：${summary.passed}/${summary.total} 通过（点击展开逐条）`}
                className={`mt-0.5 inline-flex min-h-11 shrink-0 items-center gap-1 rounded-md px-1.5 text-[11px] transition-colors hover:bg-surface-2 lg:min-h-9 ${
                  summary.allPassed ? "text-status-success" : "text-status-danger"
                }`}
              >
                {summary.allPassed ? <CheckCircle2 size={10} /> : <XCircle size={10} />}
                {summary.passed}/{summary.total}
              </button>
            )}
          </div>
          {task.spec_title && (
            <div
              className="mt-0.5 flex items-center gap-1 text-[11px] text-status-progress"
              title={`来自规范《${task.spec_title}》`}
            >
              <BookOpen size={9} className="shrink-0" />
              <span className="truncate">规范《{task.spec_title}》</span>
            </div>
          )}
          {task.failure_attribution && (
            <div
              data-status-tone={
                task.failure_attribution.repairable === false ? "danger" : "warning"
              }
              className={`mt-1 flex items-start gap-1.5 rounded-md px-2 py-1 text-[11px] leading-4 ${
                task.failure_attribution.repairable === false
                  ? "bg-status-danger-soft text-status-danger"
                  : "bg-status-warning-soft text-status-warning"
              }`}
              title={task.failure_attribution.summary}
            >
              <AlertTriangle size={9} className="mt-0.5 shrink-0" />
              <span className="shrink-0 font-medium">
                {["model-provider", "permission"].includes(task.failure_attribution.kind)
                  ? task.failure_attribution.label
                  : "系统正在恢复"}
              </span>
              <span className="min-w-0 truncate opacity-80">
                {["model-provider", "permission"].includes(task.failure_attribution.kind)
                  ? "完成必要输入后将自动续接"
                  : "正在诊断、修复并重新验证"}
              </span>
            </div>
          )}
          {task.attempts && task.attempts.length > 0 && (
            <div className="mt-0.5 text-[11px] text-gray-600">
              {task.attempts.length} 次执行记录
              {task.attempts[task.attempts.length - 1]?.status === "failed" && (
                <span className="ml-1 text-status-warning">最近一次失败</span>
              )}
            </div>
          )}
       </div>
      </div>
      {verifOpen && verif && (
        <div className="mb-1 ml-5 mr-1 space-y-0.5">
          {verif.map((r, i) => (
            <VerifCheckRow key={i} result={r} />
          ))}
        </div>
      )}
    </li>
  );
}

/** One acceptance-criterion check: ✓/✗ + name + duration, click to reveal the
 *  captured output (only when there is any). */
function VerifCheckRow({ result }: { result: VerificationResult }) {
  const [showOutput, setShowOutput] = useState(false);
  const hasOutput = result.output.trim().length > 0;
  return (
    <div className="rounded bg-surface-2">
      <div
        className={`flex min-h-11 items-center gap-1.5 px-1.5 py-0.5 lg:min-h-9 ${hasOutput ? "cursor-pointer" : ""}`}
        role={hasOutput ? "button" : undefined}
        tabIndex={hasOutput ? 0 : undefined}
        aria-expanded={hasOutput ? showOutput : undefined}
        aria-label={hasOutput ? `${result.check}，查看验收输出` : undefined}
        onClick={() => hasOutput && setShowOutput((v) => !v)}
        onKeyDown={(event) => {
          if (hasOutput && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            setShowOutput((value) => !value);
          }
        }}
      >
        {result.passed ? (
          <CheckCircle2 size={10} className="shrink-0 text-status-success" />
        ) : (
          <XCircle size={10} className="shrink-0 text-status-danger" />
        )}
        <span className="flex-1 truncate text-[12px] text-gray-400">{result.check}</span>
        <span className="text-[11px] text-gray-600">{result.duration_ms}ms</span>
      </div>
      {showOutput && hasOutput && (
        <pre className="max-h-32 overflow-y-auto whitespace-pre-wrap px-1.5 pb-1 font-mono text-[11px] text-gray-500">
          {result.output}
        </pre>
      )}
    </div>
  );
}

function statusIcon(status: string) {
  switch (status) {
    case "completed":  return CheckCircle2;
    case "running":    return Loader2;
    case "failed":     return XCircle;
    default:           return Circle;
  }
}

function statusColor(status: string): string {
  switch (status) {
    case "completed":  return "text-status-success";
    case "running":    return "text-status-progress";
    case "failed":     return "text-status-danger";
    default:           return "text-gray-600";
  }
}
