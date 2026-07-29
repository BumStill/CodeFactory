// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useState } from "react";
import { Check, CircleDot, ExternalLink, GitMerge, GitPullRequest, LoaderCircle, Rocket, X } from "lucide-react";
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

interface DeliverySnapshot {
  remote_available: boolean;
  pr: DeliveryPr | null;
  ci_status: string;
  release: DeliveryRelease | null;
  error: string | null;
}

interface Props {
  cwd: string | null;
  sessionId?: string | null;
  currentBranch: string;
  messages: UIMessage[];
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

export function WorkspaceDeliveryStatus({ cwd, sessionId, currentBranch, messages }: Props) {
  const historical = useMemo(() => deliveryReferenceFromMessages(messages), [messages]);
  const [snapshot, setSnapshot] = useState<DeliverySnapshot | null>(null);
  const [unavailable, setUnavailable] = useState(false);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!cwd) return;
    let cancelled = false;
    const args: Record<string, unknown> = {
      cwd,
      branch: historical?.branch ?? currentBranch,
      prNumber: historical?.prNumber ?? null,
    };
    if (sessionId) args.sessionId = sessionId;
    const refresh = () => invoke<DeliverySnapshot>("workspace_delivery_status", args)
      .then((next) => {
        if (!cancelled) {
          setSnapshot(next);
          setUnavailable(!next.remote_available || Boolean(next.error));
        }
      })
      .catch(() => {
        if (!cancelled) {
          setSnapshot(null);
          setUnavailable(true);
        }
      });
    void refresh();
    const id = window.setInterval(() => { void refresh(); }, 15_000);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [cwd, currentBranch, historical, sessionId]);

  if (!cwd) return null;
  const pr = snapshot?.pr;
  const ci = ciLabel(snapshot?.ci_status);
  const tone = unavailable || snapshot?.ci_status.startsWith("failure")
    ? "border-red-500/30 bg-red-500/10 text-red-700 dark:text-red-300"
    : pr?.state === "merged" || snapshot?.ci_status === "success"
      ? "border-emerald-500/25 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
      : "border-border bg-surface-2 text-gray-500";

  return (
    <>
      <button
        type="button"
        aria-label="会话交付状态"
        onClick={() => setOpen(true)}
        className={`inline-flex h-7 max-w-[390px] shrink items-center gap-1.5 overflow-hidden rounded-md border px-2 text-[11px] transition-colors hover:brightness-110 ${tone}`}
        title="查看本会话对应的 PR/MR、CI、合并、发布与线上验证状态"
      >
        <GitPullRequest size={12} className="shrink-0" />
        {unavailable ? (
          <span className="whitespace-nowrap">远程状态不可用</span>
        ) : !snapshot ? (
          <span className="inline-flex items-center gap-1 whitespace-nowrap"><LoaderCircle size={11} className="animate-spin" />读取交付状态…</span>
        ) : !pr ? (
          <span className="whitespace-nowrap">未关联 PR</span>
        ) : (
          <>
            <span className="shrink-0 font-medium">PR #{pr.number}</span>
            <StatusDivider />
            <span className="shrink-0">{ci}</span>
            <StatusDivider />
            <span className="shrink-0">{prStateLabel(pr)}</span>
            {snapshot.release && <><StatusDivider /><span className="truncate">{snapshot.release.tag} 已创建</span></>}
          </>
        )}
      </button>

      {open && (
        <div className="fixed inset-0 z-40 bg-black/30" onClick={() => setOpen(false)}>
          <aside
            role="dialog"
            aria-modal="true"
            aria-label="交付详情"
            className="absolute inset-y-0 right-0 flex w-[min(430px,94vw)] flex-col border-l border-border bg-surface-1 shadow-2xl"
            onClick={(event) => event.stopPropagation()}
          >
            <header className="flex items-start gap-3 border-b border-border px-4 py-3">
              <GitPullRequest size={16} className="mt-0.5 text-accent" />
              <div className="min-w-0 flex-1">
                <h2 className="text-sm font-semibold text-gray-100">交付详情</h2>
                <p className="mt-0.5 text-[11px] text-gray-600">状态来自会话关联的 PR/MR、CI、发布与线上验证；未验证 live 时不会标记为上线。</p>
              </div>
              <button aria-label="关闭交付详情" onClick={() => setOpen(false)} className="rounded p-1 text-gray-600 hover:bg-surface-3 hover:text-gray-200"><X size={14} /></button>
            </header>
            <div className="flex-1 overflow-y-auto p-4">
              {unavailable ? (
                <EmptyState title="远程状态不可用" detail="请检查网络，并确认该仓库已配置匹配的 Git provider、CLI 登录、远程令牌或 delivery_provider hook。" />
              ) : !pr ? (
                <EmptyState title="未关联 PR" detail="在功能分支创建或交付 PR 后，这里会显示该会话的完整交付链。" />
              ) : (
                <div className="space-y-4">
                  <section className="rounded-lg border border-border bg-surface-2 p-3">
                    <div className="flex items-start gap-2">
                      <div className="min-w-0 flex-1">
                        <div className="text-xs font-semibold text-gray-200">PR #{pr.number} · {pr.title}</div>
                        <div className="mt-1 truncate font-mono text-[11px] text-gray-500">{pr.head_branch} → {pr.base_branch}</div>
                      </div>
                      <a href={pr.url} target="_blank" rel="noreferrer" aria-label={`打开 PR #${pr.number}`} className="rounded p-1 text-gray-600 hover:bg-surface-3 hover:text-gray-200"><ExternalLink size={13} /></a>
                    </div>
                  </section>
                  <ol aria-label="交付链" className="space-y-1">
                    <DeliveryStep icon={<GitPullRequest size={13} />} label="PR" value={prStateLabel(pr)} success={pr.state === "merged" || pr.state === "open"} />
                    <DeliveryStep icon={snapshot?.ci_status === "pending" ? <LoaderCircle size={13} className="animate-spin" /> : <Check size={13} />} label="CI" value={ci} success={snapshot?.ci_status === "success"} detail={pr.head_sha.slice(0, 7)} />
                    <DeliveryStep icon={<GitMerge size={13} />} label="合并" value={pr.state === "merged" ? "已合并" : "待合并"} success={pr.state === "merged"} detail={pr.merge_commit_sha?.slice(0, 7)} />
                    <DeliveryStep icon={<Rocket size={13} />} label="发布" value={snapshot?.release ? `${snapshot.release.tag} 已创建` : "未发现包含此合并的正式版本"} success={Boolean(snapshot?.release)} />
                  </ol>
                  <p className="text-[10px] leading-relaxed text-gray-700">CI 绑定上方 PR/MR 的 head SHA；发布版本只表示 release artifact 可见，真实上线还需要 deliver_changes 的部署观察或 live verifier 通过。</p>
                </div>
              )}
            </div>
          </aside>
        </div>
      )}
    </>
  );
}

function StatusDivider() { return <span aria-hidden="true" className="text-current/40">·</span>; }

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
  return <div className="rounded-lg border border-dashed border-border px-4 py-8 text-center"><CircleDot size={18} className="mx-auto text-gray-700" /><div className="mt-2 text-xs font-medium text-gray-400">{title}</div><p className="mx-auto mt-1 max-w-xs text-[11px] leading-relaxed text-gray-600">{detail}</p></div>;
}

function DeliveryStep({ icon, label, value, success, detail }: { icon: React.ReactNode; label: string; value: string; success: boolean; detail?: string }) {
  return <li className="flex items-center gap-2 rounded-md px-2 py-2 text-[11px] hover:bg-surface-2"><span className={success ? "text-emerald-500" : "text-gray-600"}>{icon}</span><span className="w-20 text-gray-500">{label}</span><span className={success ? "text-gray-200" : "text-gray-500"}>{value}</span>{detail && <span className="ml-auto font-mono text-[10px] text-gray-700">{detail}</span>}</li>;
}
