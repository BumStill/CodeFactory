// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import {
  Check,
  CircleDot,
  ExternalLink,
  GitMerge,
  GitPullRequest,
  Globe2,
  LoaderCircle,
  Rocket,
  X,
  XCircle,
} from "lucide-react";
import { invoke } from "../lib/tauri";
import type { UIMessage } from "../stores/chat";

export interface DeliveryReference {
  branch: string;
  prNumber: number;
}

interface DeliveryPr {
  number: number;
  title: string;
  state: "open" | "closed" | "merged" | string;
  draft: boolean;
  head_branch: string;
  base_branch: string;
  head_sha: string;
  merge_commit_sha: string | null;
  url: string;
}

interface DeliveryRelease {
  tag: string;
  url: string;
  published_at: string;
}

export interface DeliverySnapshot {
  remote_available: boolean;
  pr: DeliveryPr | null;
  ci_status: string;
  release: DeliveryRelease | null;
  error: string | null;
}

export interface WorkspaceDeliveryState {
  snapshot: DeliverySnapshot | null;
  unavailable: boolean;
}

interface Props {
  cwd: string | null;
  sessionId?: string | null;
  currentBranch: string;
  messages: UIMessage[];
  onOpenDetails?: () => void;
  detailsOpen?: boolean;
  detailsId?: string;
  detailsOnly?: boolean;
  onCloseDetails?: () => void;
  deliveryState?: WorkspaceDeliveryState;
  onDeliveryStateChange?: (state: WorkspaceDeliveryState) => void;
}

/** Last successful delivery call is a compatibility fallback for conversations
 * created before session_delivery_refs existed. New calls persist this relation
 * in SQLite, so it survives returning the checkout to main. */
export function deliveryReferenceFromMessages(messages: UIMessage[]): DeliveryReference | null {
  for (let messageIndex = messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
    const calls = messages[messageIndex].toolCalls ?? [];
    for (let callIndex = calls.length - 1; callIndex >= 0; callIndex -= 1) {
      const call = calls[callIndex];
      if (call.name !== "deliver_changes" || !call.result) continue;
      const branch = call.result.match(/^分支:\s*(.+)$/m)?.[1]?.trim();
      const prNumber = Number(call.result.match(/(?:PR\s*#|\/pull\/)(\d+)/)?.[1] ?? 0);
      if (branch && prNumber > 0) return { branch, prNumber };
    }
  }
  return null;
}

export function WorkspaceDeliveryStatus({
  cwd,
  sessionId,
  currentBranch,
  messages,
  onOpenDetails,
  detailsOpen,
  detailsId,
  detailsOnly = false,
  onCloseDetails,
  deliveryState,
  onDeliveryStateChange,
}: Props) {
  const historical = useMemo(() => deliveryReferenceFromMessages(messages), [messages]);
  const requestIdentity = JSON.stringify([
    cwd,
    sessionId ?? null,
    historical?.branch ?? currentBranch,
    historical?.prNumber ?? null,
  ]);
  const [localSnapshot, setLocalSnapshot] = useState<DeliverySnapshot | null>(null);
  const [localUnavailable, setLocalUnavailable] = useState(false);
  const [localRequestIdentity, setLocalRequestIdentity] = useState(requestIdentity);
  const localStateIsCurrent = localRequestIdentity === requestIdentity;
  const snapshot = deliveryState
    ? deliveryState.snapshot
    : localStateIsCurrent ? localSnapshot : null;
  const unavailable = deliveryState
    ? deliveryState.unavailable
    : localStateIsCurrent ? localUnavailable : false;
  const [localOpen, setLocalOpen] = useState(false);
  const open = detailsOpen ?? localOpen;
  const localDialogOpen = open && !onOpenDetails;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const closeDialog = useCallback(() => {
    if (onCloseDetails) onCloseDetails();
    else setLocalOpen(false);
    requestAnimationFrame(() => triggerRef.current?.focus());
  }, [onCloseDetails]);

  useEffect(() => {
    if (!cwd || deliveryState) return;
    let cancelled = false;
    setLocalRequestIdentity(requestIdentity);
    setLocalSnapshot(null);
    setLocalUnavailable(false);
    const args: Record<string, unknown> = {
      cwd,
      branch: historical?.branch ?? currentBranch,
      prNumber: historical?.prNumber ?? null,
    };
    if (sessionId) args.sessionId = sessionId;
    const refresh = () => invoke<DeliverySnapshot>("workspace_delivery_status", args)
      .then((next) => {
        if (!cancelled) {
          const nextUnavailable = !next.remote_available || Boolean(next.error);
          setLocalSnapshot(next);
          setLocalUnavailable(nextUnavailable);
          onDeliveryStateChange?.({ snapshot: next, unavailable: nextUnavailable });
        }
      })
      .catch(() => {
        if (!cancelled) {
          setLocalSnapshot(null);
          setLocalUnavailable(true);
          onDeliveryStateChange?.({ snapshot: null, unavailable: true });
        }
      });
    void refresh();
    const id = window.setInterval(() => { void refresh(); }, 15_000);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [cwd, currentBranch, deliveryState, historical, onDeliveryStateChange, requestIdentity, sessionId]);

  useEffect(() => {
    if (!localDialogOpen || detailsOnly) return;
    const dialog = dialogRef.current;
    requestAnimationFrame(() => closeButtonRef.current?.focus());
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeDialog();
        return;
      }
      if (event.key !== "Tab" || !dialog) return;
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
  }, [closeDialog, detailsOnly, localDialogOpen]);

  if (!cwd) return null;
  const pr = snapshot?.pr;
  const ci = ciLabel(snapshot?.ci_status);
  const summary = deliverySummary(pr, ci, snapshot?.release ?? null, unavailable, snapshot);
  const statusTone = snapshot?.ci_status.startsWith("failure")
    ? "danger"
    : unavailable
      ? "warning"
      : pr
        ? "progress"
        : "neutral";
  const tone = statusTone === "danger"
    ? "border-status-danger/25 bg-status-danger-soft text-status-danger"
    : statusTone === "warning"
      ? "border-status-warning/25 bg-status-warning-soft text-status-warning"
      : statusTone === "progress"
        ? "border-status-progress/25 bg-status-progress-soft text-status-progress"
        : "border-border bg-surface-2 text-gray-500";

  if (detailsOnly) {
    return (
      <DeliveryDetailsView
        snapshot={snapshot}
        unavailable={unavailable}
        closeButtonRef={closeButtonRef}
        onClose={closeDialog}
        embedded
      />
    );
  }

  const visibleSummary = deliveryVisibleSummary(pr, snapshot, unavailable);
  const accessibleSummary = `会话交付状态；${pr ? `PR #${pr.number}；` : ""}${summary}`;

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        aria-label={accessibleSummary}
        aria-expanded={open}
        aria-controls={detailsId ?? "workspace-delivery-dialog"}
        data-status-tone={statusTone}
        onClick={() => {
          if (onOpenDetails) onOpenDetails();
          else setLocalOpen(true);
        }}
        className={`inline-flex h-11 max-w-[210px] shrink items-center gap-1.5 overflow-hidden rounded-lg border px-2 text-label transition-colors hover:brightness-95 lg:h-9 ${tone}`}
        title="查看本会话对应的 PR/MR、CI、合并、发布与线上验证状态"
      >
        <GitPullRequest size={14} className="shrink-0" />
        <span className="truncate" title={summary}>{pr ? <><span className="font-medium">PR #{pr.number}</span>{visibleSummary && <span className="ml-1.5">· {visibleSummary}</span>}</> : visibleSummary}</span>
      </button>

      {localDialogOpen && (
        <div
          className="fixed inset-0 z-40 bg-black/30"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) closeDialog();
          }}
        >
          <aside
            id="workspace-delivery-dialog"
            ref={dialogRef}
            role="dialog"
            aria-modal="true"
            aria-label="交付详情"
            className="absolute inset-y-0 right-0 flex w-[min(430px,94vw)] flex-col border-l border-border bg-surface-1 shadow-2xl"
          >
            <DeliveryDetailsView
              snapshot={snapshot}
              unavailable={unavailable}
              closeButtonRef={closeButtonRef}
              onClose={closeDialog}
            />
          </aside>
        </div>
      )}
    </>
  );
}

function DeliveryDetailsView({ snapshot, unavailable, closeButtonRef, onClose, embedded = false }: {
  snapshot: DeliverySnapshot | null;
  unavailable: boolean;
  closeButtonRef: RefObject<HTMLButtonElement>;
  onClose: () => void;
  embedded?: boolean;
}) {
  const pr = snapshot?.pr;
  const ci = ciLabel(snapshot?.ci_status);
  const content = (
    <>
      <header className="flex items-start gap-3 border-b border-border px-4 py-3">
        <GitPullRequest size={16} className="mt-0.5 text-accent" />
        <div className="min-w-0 flex-1">
          <h2 className="text-body font-semibold text-gray-100">交付详情</h2>
          <p className="mt-0.5 text-label leading-5 text-gray-600">状态来自会话关联的 PR/MR、CI、正式发布与线上验证；未验证 live 时不会标记为上线。</p>
        </div>
        <button data-auxiliary-initial-focus={embedded ? true : undefined} ref={closeButtonRef} aria-label="关闭交付详情" onClick={onClose} className="flex h-11 w-11 items-center justify-center rounded text-gray-600 hover:bg-surface-3 hover:text-gray-200 lg:h-9 lg:w-9"><X size={14} /></button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {unavailable ? (
          <EmptyState title="远程状态不可用" detail="请检查网络，并确认该仓库已配置匹配的 Git provider、CLI 登录、远程令牌或 delivery_provider hook。" />
        ) : !pr ? (
          <EmptyState title="未关联 PR" detail="在功能分支创建或交付 PR 后，这里会显示该会话的完整交付链。" />
        ) : (
          <div className="space-y-4">
            <section className="rounded-lg border border-border bg-surface-2 p-3">
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  <div className="text-note font-semibold text-gray-200">PR #{pr.number} · {pr.title}</div>
                  <div className="mt-1 truncate font-mono text-label text-gray-500">{pr.head_branch} → {pr.base_branch}</div>
                </div>
                <a href={pr.url} target="_blank" rel="noreferrer" aria-label={`打开 PR #${pr.number}`} className="rounded p-1 text-gray-600 hover:bg-surface-3 hover:text-gray-200"><ExternalLink size={14} /></a>
              </div>
            </section>
            <ol aria-label="交付链" className="space-y-1">
              <DeliveryStep icon={<GitPullRequest size={14} />} label="PR" value={prStateLabel(pr)} tone={pr.state === "merged" ? "success" : pr.state === "open" ? "progress" : "neutral"} />
              <DeliveryStep
                icon={snapshot?.ci_status === "pending" ? <LoaderCircle size={14} className="animate-spin motion-reduce:animate-none" /> : snapshot?.ci_status.startsWith("failure") ? <XCircle size={14} /> : <Check size={14} />}
                label="CI"
                value={ci}
                tone={snapshot?.ci_status === "success" ? "success" : snapshot?.ci_status === "pending" ? "progress" : snapshot?.ci_status.startsWith("failure") ? "danger" : "neutral"}
                detail={pr.head_sha.slice(0, 7)}
              />
              <DeliveryStep icon={<GitMerge size={14} />} label="合并" value={pr.state === "merged" ? "已合并" : "待合并"} tone={pr.state === "merged" ? "success" : "neutral"} detail={pr.merge_commit_sha?.slice(0, 7)} />
              <DeliveryStep icon={<Rocket size={14} />} label="正式发布" value={snapshot?.release ? `${snapshot.release.tag} 已创建` : "尚未创建包含此合并的正式版本"} tone={snapshot?.release ? "success" : "neutral"} />
              <DeliveryStep icon={<Globe2 size={14} />} label="线上验证" value={snapshot?.release ? "未验证上线" : "等待正式发布"} tone={snapshot?.release ? "warning" : "neutral"} />
            </ol>
            <p className="text-label leading-5 text-gray-600">CI 绑定上方 PR/MR 的 head SHA；正式发布只表示 release artifact 可见，真实上线还需要 deliver_changes 的部署观察或 live verifier 通过。</p>
          </div>
        )}
      </div>
    </>
  );
  return embedded
    ? <section aria-label="交付详情" className="flex min-h-0 h-full w-full flex-col bg-surface-1">{content}</section>
    : content;
}

function deliveryVisibleSummary(pr: DeliveryPr | null | undefined, snapshot: DeliverySnapshot | null, unavailable: boolean): string {
  if (unavailable) return "远程状态不可用";
  if (!snapshot) return "读取中…";
  if (!pr) return "未关联 PR";
  if (snapshot.ci_status.startsWith("failure")) return "CI 失败";
  if (snapshot.ci_status.includes("running") || snapshot.ci_status.includes("pending")) return "CI 运行中";
  if (pr.state === "merged") return snapshot.release ? "未验证上线" : "待发布";
  if (pr.draft) return "草稿";
  return snapshot.ci_status === "success" ? "待合并" : ciLabel(snapshot.ci_status);
}

function deliverySummary(pr: DeliveryPr | null | undefined, ci: string, release: DeliveryRelease | null, unavailable: boolean, snapshot: DeliverySnapshot | null): string {
  if (unavailable) return "远程状态不可用";
  if (!snapshot) return "读取交付状态…";
  if (!pr) return "未关联 PR";
  if (snapshot.ci_status.startsWith("failure")) return "CI 失败";
  if (snapshot.ci_status.includes("running") || snapshot.ci_status.includes("pending")) return "CI 运行中";
  if (pr.state === "merged") return release ? `${ci} · 已合并 · ${release.tag} · 未验证上线` : `${ci} · 已合并 · 待发布`;
  if (pr.state === "open" && snapshot.ci_status.includes("success")) return `CI 通过 · ${prStateLabel(pr)}`;
  return `${ci} · ${prStateLabel(pr)}`;
}

function ciLabel(status?: string): string {
  if (!status || status === "none") return "无 CI";
  if (status === "success") return "CI 通过";
  if (status === "pending") return "CI 运行中";
  if (status.startsWith("failure")) return "CI 失败";
  return "CI 未知";
}

function prStateLabel(pr: DeliveryPr): string {
  if (pr.state === "merged") return "已合并";
  if (pr.draft) return "草稿";
  if (pr.state === "open") return "待合并";
  return "已关闭";
}

function EmptyState({ title, detail }: { title: string; detail: string }) {
  return <div className="rounded-xl border border-dashed border-border px-4 py-8 text-center"><CircleDot size={20} className="mx-auto text-gray-600" /><div className="mt-2 text-note font-medium text-gray-400">{title}</div><p className="mx-auto mt-1 max-w-xs text-label leading-5 text-gray-600">{detail}</p></div>;
}

type DeliveryStepTone = "neutral" | "progress" | "success" | "warning" | "danger";

function deliveryStepToneClass(tone: DeliveryStepTone): string {
  if (tone === "progress") return "text-status-progress";
  if (tone === "success") return "text-status-success";
  if (tone === "warning") return "text-status-warning";
  if (tone === "danger") return "text-status-danger";
  return "text-gray-600";
}

function DeliveryStep({ icon, label, value, tone, detail }: { icon: React.ReactNode; label: string; value: string; tone: DeliveryStepTone; detail?: string }) {
  const toneClass = deliveryStepToneClass(tone);
  return <li className="flex min-h-10 items-center gap-2 rounded-lg px-2.5 text-note hover:bg-surface-2"><span className={toneClass}>{icon}</span><span className="w-20 text-gray-500">{label}</span><span className={tone === "neutral" ? "text-gray-500" : "text-gray-200"}>{value}</span>{detail && <span className="ml-auto font-mono text-caption text-gray-600">{detail}</span>}</li>;
}
