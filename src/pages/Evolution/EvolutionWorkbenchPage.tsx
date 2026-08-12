// SPDX-License-Identifier: Apache-2.0

import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  AlertTriangle,
  BrainCircuit,
  Check,
  CheckCircle2,
  ChevronRight,
  Circle,
  Clock3,
  Database,
  FileCheck2,
  FlaskConical,
  Loader2,
  Play,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldCheck,
  Zap,
  X,
  XCircle,
} from "lucide-react";

import { useChatStore } from "../../stores/chat";
import { useLearningStore, type LearningEvent } from "../../stores/learning";
import { invoke, type Session } from "../../lib/tauri";
import {
  getEvolutionJob,
  activateEvolutionCandidate,
  listEvolutionDecisionJobs,
  listEvolutionCandidateStates,
  listEvolutionEvalCaseResults,
  listEvolutionJobEvents,
  listEvolutionJobs,
  rerunEvolutionEval,
  rollbackEvolutionActivation,
  type EvolutionCandidateState,
  type EvolutionEvalCaseResult,
  type EvolutionJob,
  type EvolutionJobEvent,
} from "../../stores/evolution";

interface EvolutionWorkbenchPageProps {
  onBack: () => void;
  initialCwd?: string | null;
}

type Tab = "review" | "activation" | "jobs" | "history";

const EMPTY_EVENTS: LearningEvent[] = [];

interface ProjectMemorySnapshot {
  content: string;
  exists: boolean;
}

interface UserPreferenceSnapshot {
  cwd: string;
  key: string;
  value: string;
}

function uniqueProjectScopes(sessions: Session[]) {
  const seen = new Set<string>();
  return [...sessions]
    .filter((session) => session.kind !== "quick" && session.kind !== "anonymous")
    .sort((a, b) => b.updated_at - a.updated_at)
    .filter((session) => {
      if (!session.cwd || seen.has(session.cwd)) return false;
      seen.add(session.cwd);
      return true;
    });
}

function parseJson(value: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(value);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function asNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function evidenceSummary(event: LearningEvent): string {
  const evidence = parseJson(event.evidence_json);
  const unit = evidence.support_unit;
  const sessions = asNumber(evidence.session_count);
  const calls = asNumber(evidence.total_calls) ?? asNumber(evidence.total);
  const tasks = asNumber(evidence.task_count);
  const errors = asNumber(evidence.errors);
  const rate = asNumber(evidence.rate);
  if (unit === "sessions" && sessions != null && calls != null && errors != null && rate != null) {
    return `${sessions} 个 session · ${calls} 次调用 · ${errors} 次错误 · ${rate}%`;
  }
  if (unit === "sessions" && sessions != null && tasks != null) {
    return `${sessions} 个 session · ${tasks} 个任务反复重试`;
  }
  if (unit === "decisions") {
    const decisions = asNumber(evidence.decision_count) ?? event.support_count;
    const accepted = asNumber(evidence.accepted);
    const acceptRate = asNumber(evidence.accept_rate);
    if (accepted != null && acceptRate != null) {
      return `${decisions} 次决定 · ${accepted} 次采纳 · ${acceptRate}% 采纳率`;
    }
  }
  return event.support_count > 0 ? `${event.support_count} 条支持证据` : "单次会话观察";
}

function targetLabel(event: LearningEvent) {
  return event.kind === "preference" ? "项目偏好" : "项目记忆";
}

const AUTO_PREFERENCE_KEYS = new Set([
  "communication_style", "testing_habit", "code_style", "response_language",
]);

const AUTO_MEMORY_DENYLIST = [
  "rm -rf", "git reset --hard", "--no-verify", "bypass approval", "skip approval",
  "without approval", "auto-approve", "auto approve", "auto-merge", "auto merge",
  "automatically deploy", "automatically release", "automatically publish", "full access",
  "autonomous execution", "绕过审批", "跳过审批", "无需审批", "不需审批", "自动批准",
  "自动合并", "自动部署", "自动发布", "自动上线", "完全权限", "绕过权限", "自动执行",
];

function canOfferAutoActivation(event: LearningEvent) {
  if (event.kind === "preference") {
    const value = event.pref_value?.trim() ?? "";
    return AUTO_PREFERENCE_KEYS.has(event.pref_key ?? "")
      && value.length > 0
      && [...value].length <= 300;
  }
  const normalized = event.suggestion.trim().toLowerCase();
  return normalized.length > 0
    && [...normalized].length <= 1_200
    && !AUTO_MEMORY_DENYLIST.some((needle) => normalized.includes(needle));
}

function statusLabel(status: string) {
  const labels: Record<string, string> = {
    queued: "排队中",
    running: "运行中",
    partial: "证据不完整",
    succeeded: "已完成",
    no_candidates: "未产生候选",
    failed: "失败",
    cancelled: "已取消",
    started: "已开始",
    completed: "完成",
    waiting: "等待",
    skipped: "已跳过",
    passed: "通过",
    inconclusive: "证据不足",
    error: "运行错误",
    active: "已激活",
    rolled_back: "已回滚",
    rollback_conflict: "回滚冲突",
    pending_activation: "待激活",
    eval_failed: "评测失败",
    eval_error: "评测错误",
    eval_stale: "目标已变化，需重评",
    approved: "已批准",
  };
  return labels[status] ?? status;
}

function triggerLabel(trigger: string) {
  return ({
    cross_session: "跨会话分析",
    review_accept: "人工采纳",
    review_reject: "人工拒绝",
    review_eval: "人工批准与评测",
    eval_retry: "重新评测",
    activation: "人工激活",
    rollback: "精确回滚",
  } as Record<string, string>)[trigger] ?? trigger;
}

function shortTime(value: string | null | undefined) {
  if (!value) return "—";
  return value.replace("T", " ").slice(0, 19);
}

function sortAndDedupeJobs(groups: EvolutionJob[][]): EvolutionJob[] {
  const byId = new Map<string, EvolutionJob>();
  for (const group of groups) {
    for (const job of group) byId.set(job.id, job);
  }
  return [...byId.values()].sort((a, b) =>
    b.started_at.localeCompare(a.started_at) || b.id.localeCompare(a.id),
  );
}

export function EvolutionWorkbenchPage({ onBack, initialCwd = null }: EvolutionWorkbenchPageProps) {
  const { sessions, loadSessions } = useChatStore();
  const scopes = useMemo(() => uniqueProjectScopes(sessions), [sessions]);
  const [cwd, setCwd] = useState<string | null>(initialCwd);
  const [tab, setTab] = useState<Tab>("review");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<"accept" | "reject" | null>(null);
  const [autoActivate, setAutoActivate] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [mining, setMining] = useState(false);
  const [jobs, setJobs] = useState<EvolutionJob[]>([]);
  const [jobEvents, setJobEvents] = useState<EvolutionJobEvent[]>([]);
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [mobileDetail, setMobileDetail] = useState(false);
  const [logsLoading, setLogsLoading] = useState(false);
  const [logsError, setLogsError] = useState<string | null>(null);
  const [candidateStates, setCandidateStates] = useState<EvolutionCandidateState[]>([]);
  const [evalCases, setEvalCases] = useState<Record<string, EvolutionEvalCaseResult[]>>({});
  const [candidateStatesLoading, setCandidateStatesLoading] = useState(false);
  const [candidateStatesError, setCandidateStatesError] = useState<string | null>(null);
  const [candidateActionId, setCandidateActionId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [learningError, setLearningError] = useState<string | null>(null);
  const [currentValue, setCurrentValue] = useState<string | null>(null);
  const [currentValueLoading, setCurrentValueLoading] = useState(false);
  const [currentValueError, setCurrentValueError] = useState<string | null>(null);
  const [currentValueRevision, setCurrentValueRevision] = useState(0);
  const [decisionFocusRevision, setDecisionFocusRevision] = useState(0);
  const logsRequestId = useRef(0);
  const learningRequestId = useRef(0);
  const currentValueRequestId = useRef(0);

  const events = useLearningStore((state) =>
    cwd ? state.events[cwd] ?? EMPTY_EVENTS : EMPTY_EVENTS,
  );
  const learningLoading = useLearningStore((state) =>
    cwd ? state.loading[cwd] ?? false : false,
  );
  const load = useLearningStore((state) => state.load);
  const subscribe = useLearningStore((state) => state.subscribe);
  const accept = useLearningStore((state) => state.accept);
  const reject = useLearningStore((state) => state.reject);
  const mine = useLearningStore((state) => state.mine);

  const refreshCandidateStates = async (scope: string) => {
    setCandidateStatesLoading(true);
    setCandidateStatesError(null);
    try {
      const states = await listEvolutionCandidateStates(scope);
      const cases = await Promise.all(states.map(async (candidate) => [
        candidate.eval_run_id,
        candidate.eval_run_id
          ? await listEvolutionEvalCaseResults(scope, candidate.eval_run_id)
          : [],
      ] as const));
      setCandidateStates(states);
      setEvalCases(Object.fromEntries(cases.filter(([runId]) => runId != null)));
    } catch (reason) {
      setCandidateStatesError(String(reason));
    } finally {
      setCandidateStatesLoading(false);
    }
  };

  useEffect(() => {
    void loadSessions();
  }, [loadSessions]);

  const cwdIsValid = cwd != null && scopes.some((scope) => scope.cwd === cwd);

  useEffect(() => {
    if (scopes.length === 0) {
      if (sessions.length > 0) setCwd(null);
      return;
    }
    if (!cwd || !scopes.some((scope) => scope.cwd === cwd)) setCwd(scopes[0].cwd);
  }, [cwd, scopes]);

  const refreshLogs = async (scope: string, selectLatest = false) => {
    const requestId = ++logsRequestId.current;
    setLogsLoading(true);
    setLogsError(null);
    try {
      const [recentJobs, decisionJobs] = await Promise.all([
        listEvolutionJobs(scope),
        listEvolutionDecisionJobs(scope),
      ]);
      if (requestId !== logsRequestId.current) return;
      let nextJobs = sortAndDedupeJobs([recentJobs, decisionJobs]);
      if (!selectLatest && selectedJobId && !nextJobs.some((job) => job.id === selectedJobId)) {
        try {
          const exactJob = await getEvolutionJob(scope, selectedJobId);
          if (requestId !== logsRequestId.current) return;
          nextJobs = sortAndDedupeJobs([nextJobs, [exactJob]]);
        } catch {
          // Keep selectedJobId so the UI can truthfully show that this exact
          // source is unavailable instead of silently jumping to another job.
        }
      }
      const latestAnalysisId = nextJobs.find((job) => job.trigger === "cross_session")?.id ?? null;
      const nextSelectedJobId = !selectLatest && selectedJobId
        ? selectedJobId
        : latestAnalysisId ?? nextJobs[0]?.id ?? null;
      const eventJobIds = [...new Set(
        [nextSelectedJobId, latestAnalysisId].filter((id): id is string => id != null),
      )].filter((jobId) => nextJobs.some((job) => job.id === jobId));
      const nextEvents = (await Promise.all(
        eventJobIds.map((jobId) => listEvolutionJobEvents(scope, jobId)),
      )).flat();
      if (requestId !== logsRequestId.current) return;
      setJobs(nextJobs);
      setJobEvents(nextEvents);
      setSelectedJobId(nextSelectedJobId);
    } catch (reason) {
      if (requestId === logsRequestId.current) setLogsError(String(reason));
    } finally {
      if (requestId === logsRequestId.current) setLogsLoading(false);
    }
  };

  useEffect(() => {
    if (!cwd || !cwdIsValid) return;
    const requestId = ++learningRequestId.current;
    logsRequestId.current += 1;
    currentValueRequestId.current += 1;
    setError(null);
    setLearningError(null);
    setConfirmation(null);
    setAutoActivate(false);
    setSelectedId(null);
    setMobileDetail(false);
    setJobs([]);
    setJobEvents([]);
    setSelectedJobId(null);
    setLogsLoading(true);
    setLogsError(null);
    setCandidateStates([]);
    setEvalCases({});
    setCandidateStatesError(null);
    setCurrentValue(null);
    setCurrentValueError(null);
    setCurrentValueLoading(false);
    setCurrentValueRevision(0);
    setDecisionFocusRevision(0);
    void load(cwd).catch((reason) => {
      if (requestId === learningRequestId.current) setLearningError(String(reason));
    });
    void refreshLogs(cwd);
    void refreshCandidateStates(cwd);
    let off: (() => void) | undefined;
    let cancelled = false;
    void subscribe(cwd).then((unlisten) => {
      if (cancelled) unlisten();
      else off = unlisten;
    });
    return () => {
      cancelled = true;
      learningRequestId.current += 1;
      off?.();
    };
  }, [cwd, cwdIsValid, load, subscribe]);

  useEffect(() => {
    if (!cwd || !cwdIsValid || !jobs.some((job) => job.status === "queued" || job.status === "running")) return;
    const timer = window.setInterval(() => {
      void refreshLogs(cwd);
    }, 1_500);
    return () => window.clearInterval(timer);
  }, [cwd, cwdIsValid, jobs]);

  const pending = events.filter((event) => event.status === "pending");
  const decided = events.filter((event) => event.status !== "pending");
  const selected = pending.find((event) => event.id === selectedId) ?? pending[0] ?? null;

  useEffect(() => {
    const requestId = ++currentValueRequestId.current;
    setCurrentValue(null);
    setCurrentValueError(null);
    if (!cwd || !selected) {
      setCurrentValueLoading(false);
      return;
    }
    setCurrentValueLoading(true);
    const loadCurrentValue = async () => {
      if (selected.kind === "preference") {
        const key = selected.pref_key?.trim() || "未命名偏好";
        const existing = await invoke<UserPreferenceSnapshot | null>("get_effective_user_preference", {
          cwd,
          key: selected.pref_key ?? "",
        });
        return existing
          ? existing.cwd === cwd
            ? `项目偏好 ${key} = ${existing.value || "（空值）"}`
            : `全局偏好 ${key} = ${existing.value || "（空值）"}（当前项目未覆盖）`
          : `项目偏好 ${key} 尚未设置`;
      }
      if (!selected.suggestion.trim()) throw new Error("候选建议为空，不能采纳");
      const memory = await invoke<ProjectMemorySnapshot>("read_project_memory", { cwd });
      const marker = `<!-- codefactory-learning-event:${selected.id} -->`;
      return memory.exists && memory.content.includes(marker)
        ? "该候选已写入项目记忆，待补齐审核状态"
        : "项目记忆中尚无此条内容";
    };
    void loadCurrentValue()
      .then((value) => {
        if (requestId === currentValueRequestId.current) setCurrentValue(value);
      })
      .catch((reason) => {
        if (requestId === currentValueRequestId.current) {
          setCurrentValueError(`当前值读取失败：${String(reason)}`);
        }
      })
      .finally(() => {
        if (requestId === currentValueRequestId.current) setCurrentValueLoading(false);
      });
  }, [cwd, selected?.id, selected?.kind, selected?.pref_key, selected?.suggestion, currentValueRevision]);

  useEffect(() => {
    if (!selected) {
      setSelectedId(null);
      return;
    }
    if (selected.id !== selectedId) setSelectedId(selected.id);
  }, [selected?.id, selectedId]);

  const runAnalysis = async () => {
    if (!cwd || mining) return;
    setMining(true);
    setError(null);
    try {
      await mine(cwd);
      await refreshLogs(cwd, true);
      setTab("jobs");
    } catch (reason) {
      setError(String(reason));
      await refreshLogs(cwd, true);
      setTab("jobs");
    } finally {
      setMining(false);
    }
  };

  const confirmAccept = async () => {
    if (!cwd || !selected || currentValueLoading || currentValueError) return;
    setBusyId(selected.id);
    setError(null);
    try {
      await accept(selected.id, cwd, autoActivate);
      setConfirmation(null);
      setAutoActivate(false);
      setMobileDetail(false);
      setDecisionFocusRevision((value) => value + 1);
      await Promise.all([refreshLogs(cwd, true), refreshCandidateStates(cwd)]);
    } catch (reason) {
      setError(String(reason));
      setCurrentValueRevision((value) => value + 1);
      await refreshLogs(cwd, true);
      setTab("jobs");
    } finally {
      setBusyId(null);
    }
  };

  const rejectSelected = async () => {
    if (!cwd || !selected) return;
    setBusyId(selected.id);
    setError(null);
    try {
      await reject(selected.id, cwd);
      setConfirmation(null);
      setMobileDetail(false);
      setDecisionFocusRevision((value) => value + 1);
      await refreshLogs(cwd, true);
    } catch (reason) {
      setError(String(reason));
      setCurrentValueRevision((value) => value + 1);
      await refreshLogs(cwd, true);
      setTab("jobs");
    } finally {
      setBusyId(null);
    }
  };

  const openJob = async (jobId: string) => {
    if (!cwd) return;
    const requestId = ++logsRequestId.current;
    setSelectedJobId(jobId);
    setTab("jobs");
    setLogsLoading(true);
    setLogsError(null);
    try {
      const [exactJob, eventsForJob, recentJobs, decisionJobs] = await Promise.all([
        getEvolutionJob(cwd, jobId),
        listEvolutionJobEvents(cwd, jobId),
        listEvolutionJobs(cwd),
        listEvolutionDecisionJobs(cwd),
      ]);
      if (requestId !== logsRequestId.current) return;
      const nextJobs = sortAndDedupeJobs([recentJobs, decisionJobs, [exactJob]]);
      const latestAnalysisJob = nextJobs.find((job) => job.trigger === "cross_session") ?? null;
      const latestAnalysisEvents = latestAnalysisJob && latestAnalysisJob.id !== jobId
        ? await listEvolutionJobEvents(cwd, latestAnalysisJob.id)
        : [];
      if (requestId !== logsRequestId.current) return;
      setJobs((current) => sortAndDedupeJobs([current, nextJobs]));
      setJobEvents((current) => [
        ...current.filter((event) =>
          event.job_id !== jobId && event.job_id !== latestAnalysisJob?.id),
        ...eventsForJob,
        ...latestAnalysisEvents,
      ]);
    } catch (reason) {
      if (requestId === logsRequestId.current) {
        const message = String(reason);
        setError(message);
        setLogsError(message);
      }
    } finally {
      if (requestId === logsRequestId.current) setLogsLoading(false);
    }
  };

  const latestJob = jobs[0] ?? null;
  const latestAnalysisJob = jobs.find((job) => job.trigger === "cross_session") ?? null;
  const displayedJob = selectedJobId
    ? jobs.find((job) => job.id === selectedJobId) ?? null
    : latestJob;
  const selectedJobEvents = displayedJob
    ? jobEvents
        .filter((event) => event.job_id === displayedJob.id)
        .sort((a, b) => a.created_at.localeCompare(b.created_at))
    : [];
  const latestAnalysisEvents = latestAnalysisJob
    ? jobEvents.filter((event) => event.job_id === latestAnalysisJob.id)
    : [];
  const latestStage = (...stages: string[]) => latestAnalysisEvents
    .filter((event) => stages.includes(event.stage))
    .sort((a, b) => b.created_at.localeCompare(a.created_at))[0];
  const traceStage = latestStage("trace_read", "scope");
  const extractStage = latestStage("deduplicate", "extract", "privacy");
  const analysisRunning = jobs.some((job) =>
    job.trigger === "cross_session" && (job.status === "queued" || job.status === "running"),
  );
  const tabs: [Tab, string][] = [
    ["review", `待我审核 ${pending.length}`],
    ["activation", `评测与激活 ${candidateStates.length}`],
    ["jobs", "作业与日志"],
    ["history", `决定历史 ${decided.length}`],
  ];

  return (
    <div className="h-full min-w-0 flex flex-col bg-surface-0 text-gray-200">
      <header className="shrink-0 border-b border-border bg-surface-1 px-4 py-3 sm:px-6">
        <div className="flex flex-wrap items-center gap-3">
          <button
            onClick={onBack}
            className="rounded p-1.5 text-gray-500 transition-colors hover:bg-surface-3 hover:text-gray-200"
            aria-label="返回首页"
          >
            <ArrowLeft size={16} />
          </button>
          <div className="min-w-0 flex-1">
            <h1 className="text-heading font-semibold">进化审查</h1>
            <p className="text-label text-gray-500">审核真实轨迹候选，追溯当前项目最近 100 条决定</p>
          </div>
          <label className="min-w-0 max-w-full text-caption text-gray-500">
            项目范围
            <select
              value={cwd ?? ""}
              onChange={(event) => {
                logsRequestId.current += 1;
                currentValueRequestId.current += 1;
                setJobs([]);
                setJobEvents([]);
                setSelectedJobId(null);
                setLogsLoading(true);
                setLogsError(null);
                setCandidateStates([]);
                setEvalCases({});
                setCandidateStatesError(null);
                setCurrentValue(null);
                setCurrentValueError(null);
                setConfirmation(null);
                setMobileDetail(false);
                setCwd(event.target.value || null);
              }}
              disabled={mining || analysisRunning || busyId != null}
              className="ml-2 max-w-[min(52vw,360px)] rounded border border-border bg-surface-2 px-2 py-1.5 text-label text-gray-300"
              aria-label="项目范围"
            >
              {scopes.length === 0 && <option value="">暂无项目</option>}
              {scopes.map((scope) => (
                <option key={scope.cwd} value={scope.cwd}>{scope.cwd}</option>
              ))}
            </select>
          </label>
          <div className="text-right text-caption text-gray-500">
            <div>{pending.length} 条待审</div>
            <div>最近分析 {shortTime(latestAnalysisJob?.completed_at ?? latestAnalysisJob?.started_at)}</div>
          </div>
          <button
            onClick={() => void runAnalysis()}
            disabled={!cwdIsValid || mining || analysisRunning}
            className="flex items-center gap-1.5 rounded bg-accent px-3 py-1.5 text-label font-medium text-white transition-colors hover:bg-accent-hover disabled:opacity-50"
          >
            {mining ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />}
            {mining || analysisRunning ? "分析中" : "运行分析"}
          </button>
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-y-auto px-4 py-4 sm:px-6">
        <div className="mx-auto max-w-6xl space-y-4">
          <section aria-label="进化闭环" className="grid grid-cols-2 gap-2 md:grid-cols-3 xl:grid-cols-6">
            <StageCard icon={Database} title="轨迹采集" detail={mining ? "运行中" : traceStage ? statusLabel(traceStage.status) : "等待运行"} active={mining || traceStage?.status === "completed"} />
            <StageCard icon={Search} title="提取与去重" detail={mining ? "等待轨迹读取" : extractStage ? `${statusLabel(extractStage.status)} · ${latestAnalysisJob?.candidate_count ?? 0} 个新候选` : "尚无作业"} active={extractStage?.status === "completed"} />
            <StageCard icon={ShieldCheck} title="人工审核" detail={`${pending.length} 条待审`} active={pending.length > 0} />
            <StageCard icon={FileCheck2} title="批准决定" detail={`${candidateStates.length} 条版本化候选`} active={candidateStates.length > 0} />
            <StageCard icon={FlaskConical} title="Evals" detail={`${candidateStates.filter((candidate) => candidate.eval_status === "passed").length} 通过 · ${candidateStates.filter((candidate) => candidate.eval_status === "failed" || candidate.eval_status === "error").length} 未通过`} active={candidateStates.some((candidate) => candidate.eval_status === "passed")} />
            <StageCard icon={Zap} title="自动激活" detail={`${candidateStates.filter((candidate) => candidate.state === "active").length} 已激活 · ${candidateStates.filter((candidate) => candidate.state === "pending_activation").length} 待激活`} active={candidateStates.some((candidate) => candidate.state === "active")} />
          </section>

          <div className="rounded-lg border border-accent/30 bg-accent/5 px-3 py-2 text-label text-gray-400">
            人工批准会先冻结 revision 并运行激活安全 Evals；只有 exact revision 全部通过且本次显式选择自动激活时才影响下一次 Agent 调用。不会自动修改代码、合并、部署或发布。
          </div>

          <nav className="flex gap-1 overflow-x-auto rounded-lg border border-border bg-surface-1 p-1" aria-label="进化审查视图" role="tablist">
            {tabs.map(([value, label], index) => (
              <button
                key={value}
                id={`evolution-tab-${value}`}
                onClick={() => setTab(value)}
                onKeyDown={(event) => {
                  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
                  event.preventDefault();
                  const nextIndex = event.key === "Home"
                    ? 0
                    : event.key === "End"
                      ? tabs.length - 1
                      : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
                  setTab(tabs[nextIndex][0]);
                  const elements = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("[role='tab']");
                  elements?.[nextIndex]?.focus();
                }}
                role="tab"
                aria-selected={tab === value}
                aria-controls={`evolution-panel-${value}`}
                tabIndex={tab === value ? 0 : -1}
                className={`whitespace-nowrap rounded px-3 py-1.5 text-label transition-colors ${
                  tab === value ? "bg-surface-3 text-accent" : "text-gray-500 hover:text-gray-200"
                }`}
              >
                {label}
              </button>
            ))}
          </nav>

          {error && (
            <div role="alert" className="rounded border border-red-900/60 bg-red-950/20 px-3 py-2 text-label text-red-700 dark:text-red-300 break-words">
              {error}
            </div>
          )}

          <div
            id="evolution-panel-review"
            role="tabpanel"
            aria-labelledby="evolution-tab-review"
            hidden={tab !== "review"}
          >
          {tab === "review" && learningError && (
            <EmptyState title="候选读取失败" detail={learningError} />
          )}

          {tab === "review" && !learningError && (
            <ReviewPanel
              cwd={cwd}
              sessions={sessions}
              loading={learningLoading}
              pending={pending}
              selected={selected}
              selectedId={selectedId}
              onSelect={(id) => { setSelectedId(id); setConfirmation(null); }}
              mobileDetail={mobileDetail}
              onOpenMobileDetail={() => setMobileDetail(true)}
              onCloseMobileDetail={() => setMobileDetail(false)}
              confirmation={confirmation}
              onAskAccept={() => { setAutoActivate(false); setConfirmation("accept"); }}
              onAskReject={() => setConfirmation("reject")}
              onCancelConfirm={() => setConfirmation(null)}
              onAccept={() => void confirmAccept()}
              onReject={() => void rejectSelected()}
              onOpenJob={(jobId) => void openJob(jobId)}
              busy={busyId != null}
              currentValue={currentValue}
              currentValueLoading={currentValueLoading}
              currentValueError={currentValueError}
              decisionFocusRevision={decisionFocusRevision}
              onOpenHistory={() => setTab("history")}
              autoActivate={autoActivate}
              onAutoActivateChange={setAutoActivate}
            />
          )}

          </div>
          <div id="evolution-panel-activation" role="tabpanel" aria-labelledby="evolution-tab-activation" hidden={tab !== "activation"}>
          {tab === "activation" && (
            <EvaluationActivationPanel
              candidates={candidateStates}
              casesByRun={evalCases}
              jobIdsByCandidate={Object.fromEntries([...jobs].reverse()
                .filter((job) => job.candidate_id != null)
                .map((job) => [job.candidate_id as string, job.id]))}
              loading={candidateStatesLoading}
              error={candidateStatesError}
              busyId={candidateActionId}
              onOpenJob={(jobId) => void openJob(jobId)}
              onRetry={async (candidate) => {
                if (!cwd) return;
                setCandidateActionId(candidate.candidate_id);
                setCandidateStatesError(null);
                try {
                  await rerunEvolutionEval(cwd, candidate.candidate_id);
                  await Promise.all([refreshCandidateStates(cwd), refreshLogs(cwd, true)]);
                } catch (reason) {
                  const message = String(reason);
                  await Promise.all([refreshCandidateStates(cwd), refreshLogs(cwd, true)]);
                  setCandidateStatesError(message);
                } finally { setCandidateActionId(null); }
              }}
              onActivate={async (candidate) => {
                if (!cwd) return;
                setCandidateActionId(candidate.candidate_id);
                setCandidateStatesError(null);
                try {
                  await activateEvolutionCandidate(cwd, candidate.candidate_id);
                  await Promise.all([refreshCandidateStates(cwd), refreshLogs(cwd, true)]);
                } catch (reason) {
                  const message = String(reason);
                  await Promise.all([refreshCandidateStates(cwd), refreshLogs(cwd, true)]);
                  setCandidateStatesError(message);
                } finally { setCandidateActionId(null); }
              }}
              onRollback={async (candidate) => {
                if (!cwd || !candidate.activation_id) return;
                setCandidateActionId(candidate.candidate_id);
                setCandidateStatesError(null);
                try {
                  await rollbackEvolutionActivation(cwd, candidate.activation_id);
                  await Promise.all([refreshCandidateStates(cwd), refreshLogs(cwd, true)]);
                } catch (reason) {
                  const message = String(reason);
                  await Promise.all([refreshCandidateStates(cwd), refreshLogs(cwd, true)]);
                  setCandidateStatesError(message);
                } finally { setCandidateActionId(null); }
              }}
            />
          )}
          </div>
          <div id="evolution-panel-jobs" role="tabpanel" aria-labelledby="evolution-tab-jobs" hidden={tab !== "jobs"}>
          {tab === "jobs" && (
            <JobsPanel
              jobs={jobs}
              selectedJob={displayedJob ?? null}
              selectedJobId={selectedJobId}
              onSelectJob={(jobId) => void openJob(jobId)}
              events={selectedJobEvents}
              loading={logsLoading}
              error={logsError}
              onRefresh={() => cwd && void refreshLogs(cwd)}
            />
          )}
          </div>
          <div id="evolution-panel-history" role="tabpanel" aria-labelledby="evolution-tab-history" hidden={tab !== "history"}>
          {tab === "history" && (
            <HistoryPanel
              events={decided}
              jobs={jobs}
              onOpenJob={(jobId) => void openJob(jobId)}
            />
          )}
          </div>
        </div>
      </main>
    </div>
  );
}

function StageCard({
  icon: Icon,
  title,
  detail,
  active = false,
  disabled = false,
}: {
  icon: React.ElementType;
  title: string;
  detail: string;
  active?: boolean;
  disabled?: boolean;
}) {
  return (
    <div className={`rounded-lg border px-3 py-2 ${
      disabled ? "border-border bg-surface-1 opacity-55" : active ? "border-accent/40 bg-accent/5" : "border-border bg-surface-1"
    }`}>
      <div className="mb-1 flex items-center gap-1.5">
        <Icon size={14} className={active ? "text-accent" : "text-gray-600"} />
        <span className="text-caption font-medium text-gray-300">{title}</span>
      </div>
      <p className="text-caption text-gray-500">{detail}</p>
    </div>
  );
}

function ReviewPanel({
  cwd,
  sessions,
  loading,
  pending,
  selected,
  selectedId,
  onSelect,
  mobileDetail,
  onOpenMobileDetail,
  onCloseMobileDetail,
  confirmation,
  onAskAccept,
  onAskReject,
  onCancelConfirm,
  onAccept,
  onReject,
  onOpenJob,
  busy,
  currentValue,
  currentValueLoading,
  currentValueError,
  decisionFocusRevision,
  onOpenHistory,
  autoActivate,
  onAutoActivateChange,
}: {
  cwd: string | null;
  sessions: Session[];
  loading: boolean;
  pending: LearningEvent[];
  selected: LearningEvent | null;
  selectedId: string | null;
  onSelect: (id: string) => void;
  mobileDetail: boolean;
  onOpenMobileDetail: () => void;
  onCloseMobileDetail: () => void;
  confirmation: "accept" | "reject" | null;
  onAskAccept: () => void;
  onAskReject: () => void;
  onCancelConfirm: () => void;
  onAccept: () => void;
  onReject: () => void;
  onOpenJob: (jobId: string) => void;
  busy: boolean;
  currentValue: string | null;
  currentValueLoading: boolean;
  currentValueError: string | null;
  decisionFocusRevision: number;
  onOpenHistory: () => void;
  autoActivate: boolean;
  onAutoActivateChange: (value: boolean) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const backButtonRef = useRef<HTMLButtonElement>(null);
  const candidateRefs = useRef(new Map<string, HTMLButtonElement>());
  const acceptTriggerRef = useRef<HTMLButtonElement>(null);
  const rejectTriggerRef = useRef<HTMLButtonElement>(null);
  const completionActionRef = useRef<HTMLButtonElement>(null);
  const previousMobileDetail = useRef(mobileDetail);
  const previousConfirmation = useRef<"accept" | "reject" | null>(null);
  const previousDecisionFocusRevision = useRef(decisionFocusRevision);
  const handledDecisionFocusRevision = useRef(decisionFocusRevision);

  useEffect(() => {
    const wasOpen = previousMobileDetail.current;
    previousMobileDetail.current = mobileDetail;
    if (typeof window.matchMedia !== "function" || !window.matchMedia("(max-width: 1023px)").matches) return;
    if (!mobileDetail && !wasOpen) return;
    const frame = window.requestAnimationFrame(() => {
      if (mobileDetail) backButtonRef.current?.focus();
      else {
        const target = selectedId ? candidateRefs.current.get(selectedId) : null;
        (target
          ?? listRef.current?.querySelector<HTMLButtonElement>("button")
          ?? completionActionRef.current)?.focus();
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [mobileDetail, selectedId]);

  useEffect(() => {
    const previous = previousConfirmation.current;
    previousConfirmation.current = confirmation;
    const decisionCompleted = previousDecisionFocusRevision.current !== decisionFocusRevision;
    previousDecisionFocusRevision.current = decisionFocusRevision;
    if (!previous || confirmation) return;
    if (decisionCompleted) return;
    if (typeof window.matchMedia === "function"
      && window.matchMedia("(max-width: 1023px)").matches
      && !mobileDetail) return;
    const frame = window.requestAnimationFrame(() => {
      (previous === "accept" ? acceptTriggerRef.current : rejectTriggerRef.current)?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [confirmation, mobileDetail, decisionFocusRevision]);

  useEffect(() => {
    if (busy || handledDecisionFocusRevision.current === decisionFocusRevision) return;
    handledDecisionFocusRevision.current = decisionFocusRevision;
    const frame = window.requestAnimationFrame(() => {
      const target = selectedId ? candidateRefs.current.get(selectedId) : null;
      (target
        ?? listRef.current?.querySelector<HTMLButtonElement>("button")
        ?? completionActionRef.current)?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [busy, decisionFocusRevision]);

  if (!cwd) return <EmptyState title="请选择一个项目" detail="工作台会按项目隔离候选和审计日志。" />;
  if (loading) return <EmptyState title="正在加载候选" detail="读取本地 SQLite 中的待审记录。" loading />;
  if (!selected) return (
    <EmptyState
      title="当前项目已处理完"
      detail="没有待审候选。可运行分析，或查看已处理记录。"
      action={(
        <button
          ref={completionActionRef}
          onClick={onOpenHistory}
          className="mt-4 rounded border border-border px-3 py-1.5 text-label text-accent hover:bg-surface-3"
        >
          查看决定历史
        </button>
      )}
    />
  );
  const sourceSessionAvailable = !selected.session_id
    || sessions.some((session) => session.id === selected.session_id);
  const proposedValue = selected.kind === "preference"
    ? `${selected.pref_key ?? "偏好"} = ${selected.pref_value ?? ""}`
    : selected.suggestion;
  const autoActivationEligible = canOfferAutoActivation(selected);

  return (
    <section className="grid min-w-0 grid-cols-1 gap-3 lg:grid-cols-[minmax(240px,0.8fr)_minmax(0,1.5fr)]">
      <div className={`${mobileDetail ? "hidden lg:block" : "block"} min-w-0 max-h-[calc(100vh-280px)] overflow-y-auto rounded-lg border border-border bg-surface-1 p-2 lg:max-h-none lg:overflow-visible`}>
        <div className="px-2 pb-2 text-caption font-semibold text-gray-500">候选队列</div>
        <div ref={listRef} className="space-y-1.5">
          {pending.map((event) => (
            <button
              key={event.id}
              ref={(element) => {
                if (element) candidateRefs.current.set(event.id, element);
                else candidateRefs.current.delete(event.id);
              }}
              onClick={() => { onSelect(event.id); onOpenMobileDetail(); }}
              disabled={busy}
              aria-pressed={selectedId === event.id}
              className={`w-full min-w-0 rounded border p-3 text-left transition-colors disabled:cursor-wait disabled:opacity-60 ${
                selectedId === event.id ? "border-accent/50 bg-accent/10" : "border-border bg-surface-2 hover:border-gray-500"
              }`}
            >
              <div className="mb-1 flex items-center justify-between gap-2">
                <span className="text-caption text-accent">{targetLabel(event)}</span>
                <ChevronRight size={14} className="shrink-0 text-gray-600" />
              </div>
              <p className="break-words text-label font-medium leading-relaxed text-gray-200">{event.observation}</p>
              <p className="mt-1 text-caption text-gray-600">{evidenceSummary(event)}</p>
            </button>
          ))}
        </div>
      </div>

      <article className={`${mobileDetail ? "block" : "hidden lg:block"} min-w-0 rounded-lg border border-border bg-surface-1`}>
        <div className="border-b border-border p-4">
          <button ref={backButtonRef} onClick={onCloseMobileDetail} className="mb-3 flex items-center gap-1 text-caption text-gray-500 hover:text-gray-300 lg:hidden">
            <ArrowLeft size={14} /> 返回候选队列
          </button>
          <div className="mb-2 flex flex-wrap items-center gap-2 text-caption">
            <span className="rounded bg-accent/10 px-2 py-0.5 text-accent">待人工批准</span>
            <span className="text-gray-600">{shortTime(selected.created_at)}</span>
          </div>
          <h2 className="break-words text-title font-semibold leading-relaxed">{selected.observation}</h2>
        </div>
        <div className="space-y-4 p-4 pb-24 lg:pb-4">
          <DetailBlock title="建议变更">
            <p className="break-words text-body leading-relaxed text-gray-300">{selected.suggestion}</p>
          </DetailBlock>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <DetailBlock title="作用范围"><p className="break-all font-mono text-label text-gray-400">{cwd}</p></DetailBlock>
            <DetailBlock title="明确去向"><p className="text-label text-gray-400">{targetLabel(selected)}</p></DetailBlock>
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <DetailBlock title="来源与时间">
              <p className="break-all text-label text-gray-400">
                {selected.session_id
                  ? sourceSessionAvailable ? `session ${selected.session_id}` : "来源 session 已不可用"
                  : "跨会话聚合"} · {shortTime(selected.created_at)}
              </p>
              {selected.job_id && (
                <button
                  onClick={() => onOpenJob(selected.job_id as string)}
                  className="mt-2 rounded border border-border px-2 py-1 text-caption text-accent hover:bg-surface-3"
                >
                  查看来源作业
                </button>
              )}
            </DetailBlock>
            <DetailBlock title="风险与可逆性">
              <p className="text-label leading-relaxed text-gray-400">只影响当前项目后续会话；项目记忆和偏好可在“我的画像”中查看并撤销。</p>
            </DetailBlock>
          </div>
          <DetailBlock title="当前值 → 候选 revision">
            <div className="grid grid-cols-1 gap-2 text-label sm:grid-cols-2">
              <div className="rounded border border-border bg-surface-2 p-2 text-gray-500">
                当前：{currentValueLoading ? "正在读取…" : currentValue ?? "无法确认"}
              </div>
              <div className="break-words rounded border border-accent/30 bg-accent/5 p-2 text-gray-300">候选：{proposedValue}</div>
            </div>
            {currentValueError && <p role="alert" className="mt-2 text-caption text-red-700 dark:text-red-300">{currentValueError}；在确认真实当前值前不能采纳。</p>}
          </DetailBlock>
          <DetailBlock title="脱敏证据">
            <div className="rounded border border-border bg-surface-2 px-3 py-2 text-label text-gray-400">
              {evidenceSummary(selected)}
            </div>
          </DetailBlock>

          {confirmation ? (
            <div
              role="region"
              aria-labelledby="evolution-decision-title"
              aria-describedby="evolution-decision-description"
              onKeyDown={(event) => {
                if (event.key === "Escape" && !busy) onCancelConfirm();
              }}
              className="fixed inset-x-3 bottom-3 z-30 max-h-[calc(100dvh-1.5rem)] overflow-y-auto rounded-lg border border-amber-700/50 bg-surface-1 p-3 shadow-2xl lg:static lg:inset-auto lg:z-auto lg:max-h-none lg:overflow-visible lg:bg-amber-950/20 lg:shadow-none"
            >
              <p id="evolution-decision-title" className="text-label font-medium text-amber-700 dark:text-amber-200">
                {confirmation === "accept" ? "确认批准并运行 Evals" : "确认拒绝这个候选"}
              </p>
              <div id="evolution-decision-description" className="mt-2 space-y-1.5 text-caption text-gray-400">
                <p className="break-all">范围：{cwd} · 目标：{targetLabel(selected)}</p>
                <p>当前：{currentValueLoading ? "正在读取…" : currentValue ?? "无法确认"}</p>
                <p className="break-words">
                  {confirmation === "accept"
                    ? `批准后：冻结 revision，运行 7 项激活安全回归；此时不会立即写入。候选：${proposedValue}`
                    : "决定后：候选进入决定历史，不写入项目记忆或偏好；当前版本不支持撤销拒绝。"}
                </p>
                {confirmation === "accept" && autoActivationEligible && (
                  <label className="flex items-start gap-2 rounded border border-border bg-surface-2 p-2 text-gray-300">
                    <input
                      type="checkbox"
                      checked={autoActivate}
                      onChange={(event) => onAutoActivateChange(event.target.checked)}
                      className="mt-0.5"
                    />
                    <span>通过后自动激活此低风险候选（默认关闭；仅影响当前项目下一次 Agent 调用）</span>
                  </label>
                )}
                {confirmation === "accept" && !autoActivationEligible && (
                  <p className="rounded border border-border bg-surface-2 p-2 text-gray-500">
                    此候选不在低风险自动激活白名单内；Evals 会拒绝激活，且不会提供绕过入口。
                  </p>
                )}
                <p>不会自动修改代码、合并、部署或发布</p>
              </div>
              <div className="mt-3 flex flex-wrap gap-2">
                <button
                  onClick={confirmation === "accept" ? onAccept : onReject}
                  autoFocus
                  disabled={busy || (confirmation === "accept" && (currentValueLoading || currentValueError != null))}
                  className={`flex items-center gap-1.5 rounded px-3 py-1.5 text-label text-white disabled:opacity-50 ${confirmation === "accept" ? "bg-accent" : "bg-red-700"}`}
                >
                  {busy ? <Loader2 size={14} className="animate-spin" /> : confirmation === "accept" ? <Check size={14} /> : <X size={14} />}
                  {confirmation === "accept" ? "确认批准并运行 Evals" : "确认拒绝"}
                </button>
                <button onClick={onCancelConfirm} disabled={busy} className="rounded border border-border px-3 py-1.5 text-label text-gray-400 hover:bg-surface-3">取消</button>
              </div>
            </div>
          ) : (
            <div className="fixed inset-x-3 bottom-3 z-20 flex flex-wrap gap-2 rounded-lg border border-border bg-surface-1 p-3 shadow-2xl lg:static lg:inset-auto lg:z-auto lg:rounded-none lg:border-x-0 lg:border-b-0 lg:bg-transparent lg:p-0 lg:pt-4 lg:shadow-none">
              <button ref={acceptTriggerRef} onClick={onAskAccept} disabled={busy || currentValueLoading || currentValueError != null} className="flex items-center gap-1.5 rounded bg-accent px-3 py-1.5 text-label text-white hover:bg-accent-hover disabled:cursor-wait disabled:opacity-50">
                <Check size={14} /> 批准并运行 Evals
              </button>
              <button ref={rejectTriggerRef} onClick={onAskReject} disabled={busy} className="flex items-center gap-1.5 rounded border border-border px-3 py-1.5 text-label text-gray-400 hover:bg-surface-3 disabled:opacity-50">
                <X size={14} /> 拒绝
              </button>
            </div>
          )}
        </div>
      </article>
    </section>
  );
}

function EvaluationActivationPanel({ candidates, casesByRun, jobIdsByCandidate, loading, error, busyId, onOpenJob, onRetry, onActivate, onRollback }: {
  candidates: EvolutionCandidateState[];
  casesByRun: Record<string, EvolutionEvalCaseResult[]>;
  jobIdsByCandidate: Record<string, string>;
  loading: boolean;
  error: string | null;
  busyId: string | null;
  onOpenJob: (jobId: string) => void;
  onRetry: (candidate: EvolutionCandidateState) => Promise<void>;
  onActivate: (candidate: EvolutionCandidateState) => Promise<void>;
  onRollback: (candidate: EvolutionCandidateState) => Promise<void>;
}) {
  const [confirming, setConfirming] = useState<string | null>(null);
  if (error && candidates.length === 0) return <EmptyState title="评测与激活读取失败" detail={error} />;
  if (loading && candidates.length === 0) return <EmptyState title="正在加载评测与激活" detail="读取 exact revision、Eval case 和 activation receipt。" loading />;
  if (candidates.length === 0) return <EmptyState title="还没有版本化候选" detail="批准候选后，系统会先冻结 revision、运行激活安全 Evals，再根据你的选择进入待激活或自动激活。" />;
  return (
    <section className="space-y-3" aria-label="评测与激活记录">
      {error && (
        <p role="alert" className="rounded border border-red-900/50 bg-red-950/20 p-3 text-label text-red-700 dark:text-red-300">
          操作未完成：{error}。候选状态与作业日志已刷新。
        </p>
      )}
      {candidates.map((candidate) => {
        const cases = candidate.eval_run_id ? casesByRun[candidate.eval_run_id] ?? [] : [];
        const busy = busyId === candidate.candidate_id;
        return (
          <article key={candidate.candidate_id} className="rounded-lg border border-border bg-surface-1 p-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2 text-caption">
                  <span className="rounded bg-accent/10 px-2 py-0.5 text-accent">revision {candidate.revision}</span>
                  <span className="rounded bg-surface-3 px-2 py-0.5 text-gray-400">{statusLabel(candidate.state)}</span>
                  <span className="text-gray-600">{candidate.kind === "preference" ? "项目偏好" : "项目记忆"}</span>
                </div>
                <h2 className="mt-2 break-words text-body font-medium text-gray-200">{candidate.suggestion}</h2>
                <p className="mt-1 break-all font-mono text-caption text-gray-600">candidate {candidate.candidate_id}</p>
              </div>
              <div className="text-right text-caption text-gray-500">
                <p>{candidate.auto_activate ? "已选择 auto-if-pass" : "人工激活"}</p>
                <p>{shortTime(candidate.updated_at)}</p>
              </div>
            </div>

            <div className="mt-4 grid grid-cols-1 gap-3 lg:grid-cols-2">
              <div className="rounded border border-border bg-surface-2 p-3">
                <div className="flex items-center justify-between gap-2">
                  <h3 className="text-label font-medium text-gray-300">激活安全 Evals</h3>
                  <span className={`text-caption ${candidate.eval_status === "passed" ? "text-green-500" : candidate.eval_status === "failed" || candidate.eval_status === "error" ? "text-red-500" : "text-gray-500"}`}>
                    {candidate.eval_status ? statusLabel(candidate.eval_status) : "等待运行"}
                  </span>
                </div>
                <p className="mt-2 text-label text-gray-400">{candidate.eval_passed_count}/{candidate.eval_required_count} required cases 通过</p>
                <p className="mt-1 break-all font-mono text-caption text-gray-600">run {candidate.eval_run_id ?? "—"}</p>
                <p className="mt-1 break-all font-mono text-caption text-gray-600">manifest {candidate.eval_manifest_hash?.slice(0, 16) ?? "—"}</p>
                <ul className="mt-3 grid grid-cols-1 gap-1 sm:grid-cols-2">
                  {cases.map((testCase) => (
                    <li key={testCase.id} className="flex min-w-0 items-center gap-1.5 text-caption text-gray-400">
                      {testCase.status === "passed" ? <CheckCircle2 size={14} className="shrink-0 text-green-500" /> : <XCircle size={14} className="shrink-0 text-red-500" />}
                      <span className="break-words">{testCase.title}</span>
                    </li>
                  ))}
                </ul>
              </div>

              <div className="rounded border border-border bg-surface-2 p-3">
                <div className="flex items-center justify-between gap-2">
                  <h3 className="text-label font-medium text-gray-300">Activation receipt</h3>
                  <span className="text-caption text-gray-500">{candidate.activation_status ? statusLabel(candidate.activation_status) : "尚未激活"}</span>
                </div>
                <p className="mt-2 text-label leading-relaxed text-gray-400">
                  {candidate.state === "active"
                    ? "已对 exact revision 生效；下一次 Agent 调用读取该 active source。"
                    : candidate.state === "rolled_back"
                      ? "已按 exact receipt 回滚，后续 Agent 调用不再读取该变更。"
                      : candidate.state === "rollback_conflict"
                        ? "目标后来被用户修改，系统没有覆盖新值。"
                        : candidate.state === "eval_stale"
                          ? "目标在 Eval 后发生变化，旧通过结果已失效；重评前不会激活。"
                        : candidate.state === "pending_activation"
                          ? "Evals 已通过，等待人工激活。"
                          : "未生成成功 activation receipt，live target 保持不变。"}
                </p>
                <p className="mt-2 break-all font-mono text-caption text-gray-600">receipt {candidate.activation_id ?? "—"}</p>
                <p className="mt-1 text-caption text-gray-600">激活 {shortTime(candidate.activated_at)} · 回滚 {shortTime(candidate.rolled_back_at)}</p>
              </div>
            </div>

            <div className="sticky bottom-3 z-10 mt-3 flex flex-wrap gap-2 rounded-lg border border-border bg-surface-1/95 p-2 shadow-lg backdrop-blur lg:static lg:border-0 lg:bg-transparent lg:p-0 lg:shadow-none">
              {jobIdsByCandidate[candidate.candidate_id] && (
                <button onClick={() => onOpenJob(jobIdsByCandidate[candidate.candidate_id])} className="flex items-center gap-1.5 rounded border border-border px-3 py-1.5 text-label text-gray-300 hover:bg-surface-3">
                  <FileCheck2 size={14} /> 查看端到端作业日志
                </button>
              )}
              {(candidate.state === "eval_failed" || candidate.state === "eval_error" || candidate.state === "eval_stale") && (
                <button onClick={() => void onRetry(candidate)} disabled={busy} className="flex items-center gap-1.5 rounded border border-border px-3 py-1.5 text-label text-gray-300 hover:bg-surface-3 disabled:opacity-50">
                  {busy ? <Loader2 size={14} className="animate-spin" /> : <RefreshCw size={14} />} 重跑 exact revision
                </button>
              )}
              {candidate.state === "pending_activation" && (
                <button onClick={() => void onActivate(candidate)} disabled={busy} className="flex items-center gap-1.5 rounded bg-accent px-3 py-1.5 text-label text-white hover:bg-accent-hover disabled:opacity-50">
                  {busy ? <Loader2 size={14} className="animate-spin" /> : <Play size={14} />} 激活此 revision
                </button>
              )}
              {candidate.state === "active" && candidate.activation_id && (
                confirming === candidate.candidate_id ? (
                  <div role="region" aria-label="确认回滚" className="flex flex-wrap items-center gap-2 rounded border border-amber-700/50 bg-amber-950/20 p-2">
                    <span className="text-caption text-amber-700 dark:text-amber-200">只回滚 receipt {candidate.activation_id.slice(0, 8)}，确认？</span>
                    <button autoFocus onClick={() => { setConfirming(null); void onRollback(candidate); }} disabled={busy} className="rounded bg-red-700 px-2 py-1 text-caption text-white">确认回滚</button>
                    <button onClick={() => setConfirming(null)} disabled={busy} className="rounded border border-border px-2 py-1 text-caption text-gray-400">取消</button>
                  </div>
                ) : (
                  <button onClick={() => setConfirming(candidate.candidate_id)} disabled={busy} className="flex items-center gap-1.5 rounded border border-border px-3 py-1.5 text-label text-gray-300 hover:bg-surface-3 disabled:opacity-50">
                    <RotateCcw size={14} /> 回滚
                  </button>
                )
              )}
            </div>
          </article>
        );
      })}
    </section>
  );
}

function JobsPanel({ jobs, selectedJob, selectedJobId, onSelectJob, events, loading, error, onRefresh }: {
  jobs: EvolutionJob[];
  selectedJob: EvolutionJob | null;
  selectedJobId: string | null;
  onSelectJob: (id: string) => void;
  events: EvolutionJobEvent[];
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
}) {
  if (error) return <EmptyState title="作业读取失败" detail={error} />;
  if (loading && (jobs.length === 0 || (selectedJobId != null && selectedJob == null))) return <EmptyState title="正在加载作业" detail="读取准确的持久作业与阶段日志。" loading />;
  if (jobs.length === 0) return <EmptyState title="还没有分析作业" detail="点击“运行分析”，系统会记录范围、轨迹读取、隐私处理、信号提取、去重和待审节点。" />;
  if (selectedJobId != null && selectedJob == null) return <EmptyState title="来源作业不可用" detail={`没有在当前项目找到作业 ${selectedJobId}；不会改为展示其他作业。`} />;
  const job = selectedJob ?? jobs[0];
  return (
    <section className="grid min-w-0 grid-cols-1 gap-3 lg:grid-cols-[minmax(240px,0.75fr)_minmax(0,1.5fr)]">
      <div className="rounded-lg border border-border bg-surface-1 p-2">
        <p className="px-2 pb-2 text-caption font-semibold text-gray-500">最近作业</p>
        <div className="max-h-64 space-y-1.5 overflow-y-auto">
          {jobs.map((item) => (
            <button
              key={item.id}
              onClick={() => onSelectJob(item.id)}
              aria-pressed={item.id === job.id}
              className={`w-full rounded border p-2 text-left ${item.id === job.id ? "border-accent/40 bg-accent/5" : "border-border bg-surface-2 hover:border-gray-500"}`}
            >
              <span className="block text-label text-gray-300">{triggerLabel(item.trigger)}</span>
              <span className="mt-1 block text-caption text-gray-600">{statusLabel(item.status)} · {shortTime(item.started_at)}</span>
            </button>
          ))}
        </div>
        <div className="mt-3 border-t border-border p-2">
        <div className="mb-3 flex items-start justify-between gap-3">
          <div>
            <h2 className="text-body font-semibold">{triggerLabel(job.trigger)}</h2>
            <p className="mt-1 font-mono text-caption text-gray-600 break-all">{job.id}</p>
          </div>
          <div className="flex items-center gap-2">
            <span className="rounded bg-accent/10 px-2 py-0.5 text-caption text-accent">{statusLabel(job.status)}</span>
            <button onClick={onRefresh} aria-label="刷新作业日志" className="rounded p-1 text-gray-600 hover:bg-surface-3 hover:text-gray-300">
              <RefreshCw size={14} className={loading ? "animate-spin" : ""} />
            </button>
          </div>
        </div>
        <p className="text-label text-gray-400">{job.input_session_count} 个 session · {job.input_trace_count} 条轨迹 · {job.candidate_count} 个候选</p>
        <p className="mt-2 text-caption text-gray-600">{shortTime(job.started_at)} → {shortTime(job.completed_at)}</p>
        {job.error && <p className="mt-3 break-words rounded border border-red-900/50 bg-red-950/20 p-2 text-label text-red-700 dark:text-red-300">{job.error}</p>}
        </div>
      </div>
      <div className="rounded-lg border border-border bg-surface-1 p-4">
        <h2 className="mb-3 text-label font-semibold text-gray-500">结构化作业日志</h2>
        {loading && events.length === 0 ? (
          <EmptyState title="正在加载阶段日志" detail="按当前作业读取最近阶段日志，并保留最新终态。" loading />
        ) : events.length === 0 ? (
          <EmptyState title="该作业没有阶段日志" detail="旧记录可能没有阶段日志；系统不会为它补造时间线。" />
        ) : <>
          {events.length === 500 && (
            <p className="mb-3 rounded border border-amber-800/50 bg-amber-950/20 px-2 py-1.5 text-caption text-amber-700 dark:text-amber-200">
              当前已达显示上限，仅展示最近 500 条阶段日志并保留最新终态。
            </p>
          )}
          <ol className="space-y-3">
          {events.map((event) => (
            <li key={event.id} className="flex min-w-0 gap-3">
              <div className="mt-0.5 shrink-0">
                <JobEventIcon status={event.status} />
              </div>
              <div className="min-w-0 flex-1 border-b border-border pb-3">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p className="break-words text-label font-medium text-gray-300">{event.title}</p>
                  <span className="text-caption text-gray-600">{statusLabel(event.status)} · {shortTime(event.created_at)}</span>
                </div>
                <EventDetails value={event.detail_json} />
              </div>
            </li>
          ))}
          </ol>
        </>}
      </div>
    </section>
  );
}

function JobEventIcon({ status }: { status: string }) {
  if (status === "failed") return <XCircle size={14} className="text-red-500" aria-label="失败" />;
  if (status === "waiting") return <Clock3 size={14} className="text-amber-500" aria-label="等待" />;
  if (status === "started") return <Clock3 size={14} className="text-accent" aria-label="已开始" />;
  if (status === "running") return <Loader2 size={14} className="animate-spin text-accent" aria-label="运行中" />;
  if (status === "partial") return <AlertTriangle size={14} className="text-amber-500" aria-label="证据不完整" />;
  if (status === "cancelled" || status === "skipped") return <Circle size={14} className="text-gray-600" aria-label={status === "cancelled" ? "已取消" : "已跳过"} />;
  if (status === "completed" || status === "succeeded") return <CheckCircle2 size={14} className="text-green-500" aria-label="完成" />;
  return <Circle size={14} className="text-gray-600" aria-label={`未知状态 ${status}`} />;
}

function EventDetails({ value }: { value: string }) {
  const details = parseJson(value);
  const allowed = new Set([
    "schema_version", "session_count", "trace_count", "tool_call_count", "task_run_count",
    "decision_count", "candidate_count", "extracted_count", "duplicate_count", "pending_count",
    "support_count", "status", "terminal_status", "candidate_status", "candidate_kind", "decision", "target", "trigger", "reason", "error",
    "aggregate_only", "redactor", "reasoning_included", "raw_prompt_included",
    "already_present", "candidate_marker_present", "value_persisted", "materialization_started",
    "candidate_id", "revision", "auto_activate", "run_id", "manifest_hash",
    "required_count", "passed_count", "failed_count", "verdict", "activation_id",
    "before_hash", "after_hash",
  ]);
  const entries = Object.entries(details).filter(([key]) => allowed.has(key));
  if (entries.length === 0) return null;
  const labels: Record<string, string> = {
    schema_version: "日志版本",
    session_count: "会话数",
    trace_count: "轨迹数",
    tool_call_count: "工具调用数",
    task_run_count: "任务数",
    decision_count: "历史决定数",
    candidate_count: "候选数",
    extracted_count: "提取数",
    duplicate_count: "去重数",
    pending_count: "待审数",
    support_count: "支持证据数",
    status: "状态",
    terminal_status: "终态",
    candidate_status: "候选状态",
    candidate_kind: "候选类型",
    decision: "人工决定",
    target: "写入目标",
    trigger: "触发方式",
    reason: "原因",
    error: "错误摘要",
    aggregate_only: "仅聚合数据",
    redactor: "脱敏方式",
    reasoning_included: "包含模型推理",
    raw_prompt_included: "包含原始提示词",
    already_present: "已存在",
    candidate_marker_present: "候选标记已写入",
    value_persisted: "值已持久化",
    materialization_started: "已开始物化",
    candidate_id: "候选 ID",
    revision: "候选版本",
    auto_activate: "通过后自动激活",
    run_id: "Eval run",
    manifest_hash: "Eval 清单哈希",
    required_count: "必过用例数",
    passed_count: "通过用例数",
    failed_count: "失败用例数",
    verdict: "Eval 结论",
    activation_id: "Activation receipt",
    before_hash: "激活前指纹",
    after_hash: "激活后指纹",
  };
  const namedValues: Record<string, string> = {
    cross_session: "跨会话分析",
    review_accept: "人工采纳",
    review_reject: "人工拒绝",
    review_eval: "人工批准与 Evals",
    eval_retry: "重跑 Evals",
    activation: "人工激活",
    rollback: "精确回滚",
    process_restart: "应用重启",
    memory: "项目记忆",
    preference: "项目偏好",
    pattern: "跨会话模式",
    trajectory: "轨迹脱敏",
    accepted: "已采纳",
    rejected: "已拒绝",
  };
  return (
    <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-caption text-gray-600">
      {entries.map(([key, raw]) => {
        const primitive = typeof raw === "string" || typeof raw === "number" || typeof raw === "boolean" ? String(raw).slice(0, 160) : "已记录";
        const rendered = typeof raw === "boolean"
          ? raw ? "是" : "否"
          : key.includes("status") ? statusLabel(primitive)
            : namedValues[primitive] ?? primitive;
        return <span key={key} className="max-w-full break-all">{labels[key] ?? key}：{rendered}</span>;
      })}
    </div>
  );
}

function HistoryPanel({ events, jobs, onOpenJob }: {
  events: LearningEvent[];
  jobs: EvolutionJob[];
  onOpenJob: (jobId: string) => void;
}) {
  if (events.length === 0) return <EmptyState title="还没有人工决定" detail="采纳或拒绝候选后，会在这里保留结果。" />;
  return (
    <section className="overflow-hidden rounded-lg border border-border bg-surface-1">
      <ul className="divide-y divide-border">
        {events.map((event) => {
          const decisionTrigger = event.status === "accepted" ? "review_accept" : "review_reject";
          const decisionJob = jobs.find((job) =>
            job.candidate_id === event.id && job.trigger === decisionTrigger,
          );
          return <li key={event.id} className="flex min-w-0 items-start gap-3 p-4">
            {event.status === "accepted" ? <CheckCircle2 size={14} className="mt-0.5 shrink-0 text-green-500" /> : <XCircle size={14} className="mt-0.5 shrink-0 text-gray-600" />}
            <div className="min-w-0 flex-1">
              <p className="break-words text-label text-gray-300">{event.observation}</p>
              <p className="mt-1 text-caption text-gray-600">{event.status === "accepted" ? `历史已生效（未评测） · ${targetLabel(event)}` : "已拒绝"} · {shortTime(event.decided_at)}</p>
              <p className="mt-2 break-words text-label leading-relaxed text-gray-500">{event.suggestion}</p>
              <p className="mt-1 text-caption text-gray-600">{evidenceSummary(event)}</p>
              <div className="mt-2 flex flex-wrap gap-2">
                {event.job_id && (
                  <button onClick={() => onOpenJob(event.job_id as string)} className="rounded border border-border px-2 py-1 text-caption text-accent hover:bg-surface-3">
                    查看来源作业
                  </button>
                )}
                {decisionJob && (
                  <button onClick={() => onOpenJob(decisionJob.id)} className="rounded border border-border px-2 py-1 text-caption text-accent hover:bg-surface-3">
                    {event.status === "accepted" ? "查看历史审核与物化日志" : "查看拒绝审核日志"}
                  </button>
                )}
                {!event.job_id && !decisionJob && (
                  <span className="text-caption text-gray-600">历史记录，无阶段日志</span>
                )}
              </div>
            </div>
          </li>;
        })}
      </ul>
    </section>
  );
}

function DetailBlock({ title, children }: { title: string; children: React.ReactNode }) {
  return <div className="min-w-0"><h3 className="mb-1.5 text-caption font-semibold text-gray-500">{title}</h3>{children}</div>;
}

function EmptyState({ title, detail, loading = false, action }: {
  title: string;
  detail: string;
  loading?: boolean;
  action?: React.ReactNode;
}) {
  return (
    <div className="rounded-lg border border-dashed border-border bg-surface-1 px-6 py-12 text-center">
      {loading ? <Loader2 size={20} className="mx-auto mb-3 animate-spin text-accent" /> : <BrainCircuit size={20} className="mx-auto mb-3 text-gray-600" />}
      <p className="text-body font-medium text-gray-300">{title}</p>
      <p className="mx-auto mt-1 max-w-lg text-label leading-relaxed text-gray-500">{detail}</p>
      {action}
    </div>
  );
}
