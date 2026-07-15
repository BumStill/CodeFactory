// SPDX-License-Identifier: Apache-2.0

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { EvolutionWorkbenchPage } from "./EvolutionWorkbenchPage";

const candidate = {
  id: "candidate-1",
  session_id: "",
  cwd: "/proj",
  observation: "bash 在多个会话中反复失败",
  suggestion: "执行 bash 前先检查命令和工作目录。",
  status: "pending",
  created_at: "2026-07-15T00:00:00Z",
  decided_at: null,
  kind: "pattern",
  pref_key: null,
  pref_value: null,
  support_count: 2,
  evidence_json: JSON.stringify({
    detector: "tool_reliability",
    support_unit: "sessions",
    session_count: 2,
    total_calls: 8,
    errors: 2,
    rate: 25,
  }),
  job_id: "job-1",
};

const decidedCandidate = {
  ...candidate,
  id: "candidate-decided",
  observation: "已处理的旧候选",
  suggestion: "保留决定审计链。",
  status: "accepted",
  decided_at: "2026-07-15T01:00:00Z",
  job_id: "job-source-old",
};

let projectEvents = [candidate, decidedCandidate];

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  load: vi.fn(async () => {}),
  subscribe: vi.fn(async () => () => {}),
  accept: vi.fn(async () => {}),
  reject: vi.fn(async () => {}),
  mine: vi.fn(async () => 1),
}));

vi.mock("../../lib/tauri", () => ({ invoke: mocks.invoke }));

vi.mock("../../stores/chat", () => ({
  useChatStore: () => ({
    sessions: [
      {
        id: "session-1",
        title: "CodeFactory",
        cwd: "/proj",
        model_id: "test",
        created_at: 1,
        updated_at: 2,
        total_input_tokens: 0,
        total_output_tokens: 0,
        kind: "project",
      },
      {
        id: "session-2",
        title: "Other",
        cwd: "/other",
        model_id: "test",
        created_at: 1,
        updated_at: 1,
        total_input_tokens: 0,
        total_output_tokens: 0,
        kind: "project",
      },
    ],
    loadSessions: vi.fn(),
  }),
}));

vi.mock("../../stores/learning", () => ({
  useLearningStore: (selector: (state: unknown) => unknown) =>
    selector({
      events: { "/proj": projectEvents, "/other": [] },
      loading: { "/proj": false, "/other": false },
      load: mocks.load,
      subscribe: mocks.subscribe,
      accept: mocks.accept,
      reject: mocks.reject,
      mine: mocks.mine,
    }),
}));

describe("EvolutionWorkbenchPage", () => {
  beforeEach(() => {
    projectEvents = [candidate, decidedCandidate];
    mocks.invoke.mockReset();
    mocks.load.mockClear();
    mocks.subscribe.mockClear();
    mocks.accept.mockReset();
    mocks.accept.mockResolvedValue(undefined);
    mocks.reject.mockReset();
    mocks.reject.mockResolvedValue(undefined);
    mocks.mine.mockReset();
    mocks.mine.mockResolvedValue(1);
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "existing fact", exists: true };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_decision_jobs") {
        return [{
          id: "job-decision-old",
          cwd: "/proj",
          trigger: "review_accept",
          candidate_id: "candidate-decided",
          status: "succeeded",
          input_session_count: 0,
          input_trace_count: 0,
          candidate_count: 1,
          started_at: "2026-07-15T01:00:00Z",
          completed_at: "2026-07-15T01:00:01Z",
          error: null,
        }];
      }
      if (command === "get_evolution_job") {
        return {
          id: "job-1",
          cwd: "/proj",
          trigger: "cross_session",
          candidate_id: null,
          status: "succeeded",
          input_session_count: 2,
          input_trace_count: 8,
          candidate_count: 1,
          started_at: "2026-07-15T00:00:00Z",
          completed_at: "2026-07-15T00:00:01Z",
          error: null,
        };
      }
      if (command === "list_evolution_jobs") {
        return [
          {
            id: "job-1",
            cwd: "/proj",
            trigger: "cross_session",
            status: "succeeded",
            input_session_count: 2,
            input_trace_count: 8,
            candidate_count: 1,
            started_at: "2026-07-15T00:00:00Z",
            completed_at: "2026-07-15T00:00:01Z",
            error: null,
          },
        ];
      }
      if (command === "list_evolution_job_events") {
        return [
          {
            id: "log-1",
            cwd: "/proj",
            job_id: "job-1",
            candidate_id: null,
            stage: "trace_read",
            status: "started",
            title: "轨迹读取完成",
            detail_json: JSON.stringify({ session_count: 2, trace_count: 8 }),
            created_at: "2026-07-15T00:00:00Z",
          },
          {
            id: "log-2",
            cwd: "/proj",
            job_id: "job-1",
            candidate_id: "candidate-1",
            stage: "review",
            status: "waiting",
            title: "等待人工审核",
            detail_json: "{}",
            created_at: "2026-07-15T00:00:01Z",
          },
        ];
      }
      return [];
    });
  });

  it("makes the human review surface explicit and keeps materialization truthful", async () => {
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    expect(await screen.findByRole("heading", { name: "进化审查" })).toBeInTheDocument();
    expect(screen.getAllByText("bash 在多个会话中反复失败")).toHaveLength(2);
    expect(screen.getAllByText("2 个 session · 8 次调用 · 2 次错误 · 25%")).toHaveLength(2);
    expect(screen.getByText(/Evals 与自动激活尚未接入/)).toBeInTheDocument();
    expect(await screen.findByText("当前：项目记忆中尚无此条内容")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "采纳并写入项目记忆" }));
    expect(screen.getByText("不会自动合并、部署或发布")).toBeInTheDocument();
    expect(screen.getByText(/决定后：执行 bash 前先检查命令和工作目录/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "确认采纳并写入项目记忆" }));

    await waitFor(() => expect(mocks.accept).toHaveBeenCalledWith("candidate-1", "/proj"));
  });

  it("requires an explicit second step before rejecting a candidate", async () => {
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.click(await screen.findByRole("button", { name: "拒绝" }));
    expect(mocks.reject).not.toHaveBeenCalled();
    expect(screen.getByText("确认拒绝这个候选")).toBeInTheDocument();
    expect(screen.getByText(/不写入项目记忆或偏好/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "确认拒绝" }));
    await waitFor(() => expect(mocks.reject).toHaveBeenCalledWith("candidate-1", "/proj"));
  });

  it("blocks acceptance when the real current value cannot be read but still allows rejection", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") throw new Error("memory unavailable");
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs" || command === "list_evolution_job_events") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    expect(await screen.findByText(/当前值读取失败.*memory unavailable/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "采纳并写入项目记忆" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "拒绝" })).toBeEnabled();
  });

  it("does not mistake a suggestion substring for an existing materialized candidate", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") {
        return {
          path: "/proj/.codefactory/memory.md",
          content: `unrelated prose contains ${candidate.suggestion} but no candidate marker`,
          exists: true,
        };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs" || command === "list_evolution_job_events" || command === "list_evolution_decision_jobs") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    expect(await screen.findByText("当前：项目记忆中尚无此条内容")).toBeInTheDocument();
  });

  it("keeps an exact decision log link even when the job is outside the recent list", async () => {
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);
    await userEvent.click(await screen.findByRole("tab", { name: /决定历史/ }));

    expect(screen.getByText("已处理的旧候选")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "查看审核与物化日志" })).toBeInTheDocument();
  });

  it("opens the exact persisted source job from candidate details", async () => {
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    const sourceJobButton = await screen.findByRole("button", { name: "查看来源作业" });
    mocks.invoke.mockClear();
    await userEvent.click(sourceJobButton);

    expect(await screen.findByRole("tab", { name: "作业与日志" })).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith(
      "get_evolution_job",
      { cwd: "/proj", jobId: "job-1" },
    ));
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith(
      "list_evolution_job_events",
      { cwd: "/proj", jobId: "job-1" },
    ));
  });

  it("loads the exact old source job instead of falling back to the latest job", async () => {
    mocks.invoke.mockImplementation(async (command: string, args?: { jobId?: string }) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs") return [
        {
          id: "job-latest", cwd: "/proj", trigger: "review_reject", candidate_id: "other",
          status: "succeeded", input_session_count: 0, input_trace_count: 0, candidate_count: 1,
          started_at: "2026-07-15T04:00:00Z", completed_at: "2026-07-15T04:00:01Z", error: null,
        },
        {
          id: "job-latest-analysis", cwd: "/proj", trigger: "cross_session", candidate_id: null,
          status: "succeeded", input_session_count: 4, input_trace_count: 20, candidate_count: 2,
          started_at: "2026-07-15T03:00:00Z", completed_at: "2026-07-15T03:00:01Z", error: null,
        },
      ];
      if (command === "get_evolution_job" && args?.jobId === "job-1") return {
        id: "job-1", cwd: "/proj", trigger: "cross_session", candidate_id: null,
        status: "succeeded", input_session_count: 2, input_trace_count: 8, candidate_count: 1,
        started_at: "2026-07-15T00:00:00Z", completed_at: "2026-07-15T00:00:01Z", error: null,
      };
      if (command === "list_evolution_job_events") return [];
      if (command === "list_evolution_decision_jobs") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.click(await screen.findByRole("button", { name: "查看来源作业" }));

    expect(await screen.findByText("job-1")).toBeInTheDocument();
    expect(screen.queryByText("job-latest")).not.toBeInTheDocument();
    expect(screen.getByText("最近分析 2026-07-15 03:00:01")).toBeInTheDocument();
  });

  it("shows the persisted end-to-end analysis job and structured logs", async () => {
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.click(await screen.findByRole("tab", { name: "作业与日志" }));

    expect(await screen.findByRole("heading", { name: "跨会话分析" })).toBeInTheDocument();
    expect(screen.getByText("2 个 session · 8 条轨迹 · 1 个候选")).toBeInTheDocument();
    expect(screen.getByText("轨迹读取完成")).toBeInTheDocument();
    expect(screen.getByText("等待人工审核")).toBeInTheDocument();
    expect(screen.getByLabelText("已开始")).toBeInTheDocument();
  });

  it("shows a log read failure instead of a fake empty ledger", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs") throw new Error("ledger unavailable");
      if (command === "list_evolution_decision_jobs") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.click(await screen.findByRole("tab", { name: "作业与日志" }));

    expect(await screen.findByText("作业读取失败")).toBeInTheDocument();
    expect(screen.getByText(/ledger unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("还没有分析作业")).not.toBeInTheDocument();
  });

  it("keeps an unavailable exact source selected after refresh", async () => {
    let exactReads = 0;
    mocks.invoke.mockImplementation(async (command: string, args?: { jobId?: string }) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs") return [{
        id: "job-latest", cwd: "/proj", trigger: "cross_session", status: "succeeded",
        input_session_count: 3, input_trace_count: 10, candidate_count: 1,
        started_at: "2026-07-15T03:00:00Z", completed_at: "2026-07-15T03:00:01Z", error: null,
      }];
      if (command === "list_evolution_decision_jobs") return [];
      if (command === "get_evolution_job" && args?.jobId === "job-1") {
        exactReads += 1;
        if (exactReads > 1) throw new Error("source deleted");
        return {
          id: "job-1", cwd: "/proj", trigger: "cross_session", status: "succeeded",
          input_session_count: 2, input_trace_count: 8, candidate_count: 1,
          started_at: "2026-07-15T00:00:00Z", completed_at: "2026-07-15T00:00:01Z", error: null,
        };
      }
      if (command === "list_evolution_job_events") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);
    await userEvent.click(await screen.findByRole("button", { name: "查看来源作业" }));
    expect(await screen.findByText("job-1")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "刷新作业日志" }));

    expect(await screen.findByText("来源作业不可用")).toBeInTheDocument();
    expect(screen.getByText(/job-1/)).toBeInTheDocument();
    expect(screen.queryByText("job-latest")).not.toBeInTheDocument();
  });

  it("loads latest analysis events while opening an older exact job", async () => {
    let resolveInitialJobs!: (value: unknown[]) => void;
    const initialJobs = new Promise<unknown[]>((resolve) => { resolveInitialJobs = resolve; });
    let jobListReads = 0;
    mocks.invoke.mockImplementation(async (command: string, args?: { jobId?: string }) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs") {
        jobListReads += 1;
        if (jobListReads === 1) return initialJobs;
        return [{
          id: "job-latest-analysis", cwd: "/proj", trigger: "cross_session", status: "succeeded",
          input_session_count: 3, input_trace_count: 10, candidate_count: 1,
          started_at: "2026-07-15T03:00:00Z", completed_at: "2026-07-15T03:00:01Z", error: null,
        }];
      }
      if (command === "list_evolution_decision_jobs") return [];
      if (command === "get_evolution_job") return {
        id: args?.jobId, cwd: "/proj", trigger: "review_accept", candidate_id: "candidate-1",
        status: "succeeded", input_session_count: 0, input_trace_count: 0, candidate_count: 1,
        started_at: "2026-07-15T00:00:00Z", completed_at: "2026-07-15T00:00:01Z", error: null,
      };
      if (command === "list_evolution_job_events" && args?.jobId === "job-latest-analysis") return [{
        id: "latest-scope", cwd: "/proj", job_id: "job-latest-analysis", candidate_id: null,
        stage: "scope", status: "completed", title: "分析范围已确定", detail_json: "{}",
        created_at: "2026-07-15T03:00:00Z",
      }];
      if (command === "list_evolution_job_events") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.click(await screen.findByRole("button", { name: "查看来源作业" }));

    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith(
      "list_evolution_job_events",
      { cwd: "/proj", jobId: "job-latest-analysis" },
    ));
    await act(async () => resolveInitialJobs([]));
  });

  it("opens the persisted failed analysis after a run error", async () => {
    let jobReads = 0;
    mocks.mine.mockRejectedValueOnce(new Error("analysis failed"));
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_decision_jobs" || command === "list_evolution_job_events") return [];
      if (command === "list_evolution_jobs") {
        jobReads += 1;
        return jobReads === 1 ? [] : [{
          id: "job-failed", cwd: "/proj", trigger: "cross_session", status: "failed",
          input_session_count: 2, input_trace_count: 8, candidate_count: 0,
          started_at: "2026-07-15T04:00:00Z", completed_at: "2026-07-15T04:00:01Z",
          error: "analysis failed",
        }];
      }
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.click(await screen.findByRole("button", { name: "运行分析" }));

    expect(await screen.findByText("job-failed")).toBeInTheDocument();
    expect(screen.getByText("analysis failed")).toBeInTheDocument();
  });

  it("refreshes the real current value and failed receipt after an accept error", async () => {
    let memoryReads = 0;
    let jobReads = 0;
    mocks.accept.mockRejectedValueOnce(new Error("terminal write failed"));
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") {
        memoryReads += 1;
        return {
          path: "/proj/.codefactory/memory.md",
          content: memoryReads > 1 ? `<!-- codefactory-learning-event:${candidate.id} -->` : "",
          exists: memoryReads > 1,
        };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_decision_jobs" || command === "list_evolution_job_events") return [];
      if (command === "list_evolution_jobs") {
        jobReads += 1;
        return jobReads === 1 ? [] : [{
          id: "job-accept-failed", cwd: "/proj", trigger: "review_accept",
          candidate_id: candidate.id, status: "failed", input_session_count: 0,
          input_trace_count: 0, candidate_count: 0, started_at: "2026-07-15T05:00:00Z",
          completed_at: "2026-07-15T05:00:01Z", error: "terminal write failed",
        }];
      }
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);
    await userEvent.click(await screen.findByRole("button", { name: "采纳并写入项目记忆" }));
    await userEvent.click(screen.getByRole("button", { name: "确认采纳并写入项目记忆" }));

    expect(await screen.findByText("job-accept-failed")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: /待我审核/ }));
    expect(await screen.findAllByText("当前：该候选已写入项目记忆，待补齐审核状态"))
      .not.toHaveLength(0);
  });

  it("focuses the decision-history action after completing the final candidate", async () => {
    mocks.accept.mockImplementationOnce(async () => {
      projectEvents = [{ ...candidate, status: "accepted", decided_at: "2026-07-15T05:00:00Z" }];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);
    await userEvent.click(await screen.findByRole("button", { name: "采纳并写入项目记忆" }));
    await userEvent.click(screen.getByRole("button", { name: "确认采纳并写入项目记忆" }));

    const historyAction = await screen.findByRole("button", { name: "查看决定历史" });
    await waitFor(() => expect(historyAction).toHaveFocus());
  });

  it("waits until a decision is no longer busy before focusing the next candidate", async () => {
    const nextCandidate = {
      ...candidate,
      id: "candidate-2",
      observation: "下一条待审核候选",
      suggestion: "下一条建议。",
      job_id: "job-2",
    };
    projectEvents = [candidate, nextCandidate];
    let resolveDecisionRefresh!: (value: unknown[]) => void;
    const decisionRefresh = new Promise<unknown[]>((resolve) => {
      resolveDecisionRefresh = resolve;
    });
    let jobReads = 0;
    mocks.accept.mockImplementationOnce(async () => {
      projectEvents = [{ ...candidate, status: "accepted" }, nextCandidate];
    });
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs") {
        jobReads += 1;
        return jobReads === 1 ? [] : decisionRefresh;
      }
      if (command === "list_evolution_decision_jobs" || command === "list_evolution_job_events") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);
    await userEvent.click(await screen.findByRole("button", { name: "采纳并写入项目记忆" }));
    await userEvent.click(screen.getByRole("button", { name: "确认采纳并写入项目记忆" }));

    const nextButton = await screen.findByRole("button", { name: /下一条待审核候选/ });
    expect(nextButton).toBeDisabled();
    await act(async () => resolveDecisionRefresh([]));

    await waitFor(() => expect(nextButton).toBeEnabled());
    await waitFor(() => expect(nextButton).toHaveFocus());
  });

  it("shows a source-job read error instead of an empty ledger", async () => {
    let resolveInitialJobs!: (value: unknown[]) => void;
    const initialJobs = new Promise<unknown[]>((resolve) => {
      resolveInitialJobs = resolve;
    });
    let jobReads = 0;
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "read_project_memory") {
        return { path: "/proj/.codefactory/memory.md", content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (command === "list_evolution_jobs") {
        jobReads += 1;
        return jobReads === 1 ? initialJobs : [];
      }
      if (command === "list_evolution_decision_jobs" || command === "list_evolution_job_events") return [];
      if (command === "get_evolution_job") throw new Error("source ledger unavailable");
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.click(await screen.findByRole("button", { name: "查看来源作业" }));

    expect(await screen.findByText("作业读取失败")).toBeInTheDocument();
    expect(screen.getAllByText(/source ledger unavailable/)).toHaveLength(2);
    expect(screen.queryByText("还没有分析作业")).not.toBeInTheDocument();
    await act(async () => resolveInitialJobs([]));
  });

  it("does not turn a candidate query failure into a fake all-clear state", async () => {
    mocks.load.mockRejectedValueOnce(new Error("sqlite unavailable"));

    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    expect(await screen.findByText("候选读取失败")).toBeInTheDocument();
    expect(screen.getByText(/sqlite unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("当前项目已处理完")).not.toBeInTheDocument();
  });

  it("discards late log responses from a previously selected project", async () => {
    let resolveOldJobs!: (value: unknown[]) => void;
    let resolveOldEvents!: (value: unknown[]) => void;
    const oldJobs = new Promise<unknown[]>((resolve) => { resolveOldJobs = resolve; });
    const oldEvents = new Promise<unknown[]>((resolve) => { resolveOldEvents = resolve; });
    mocks.invoke.mockImplementation(async (command: string, args?: { cwd?: string }) => {
      if (args?.cwd === "/proj") {
        return command === "list_evolution_jobs" ? oldJobs : oldEvents;
      }
      if (command === "list_evolution_jobs") {
        return [{
          id: "job-other",
          cwd: "/other",
          trigger: "cross_session",
          status: "succeeded",
          input_session_count: 1,
          input_trace_count: 3,
          candidate_count: 0,
          started_at: "2026-07-15T01:00:00Z",
          completed_at: "2026-07-15T01:00:01Z",
          error: null,
        }];
      }
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);

    await userEvent.selectOptions(screen.getByLabelText("项目范围"), "/other");
    await userEvent.click(screen.getByRole("tab", { name: "作业与日志" }));
    expect(await screen.findByText("job-other")).toBeInTheDocument();

    resolveOldJobs([{
      id: "job-stale",
      cwd: "/proj",
      trigger: "cross_session",
      status: "failed",
      input_session_count: 99,
      input_trace_count: 99,
      candidate_count: 99,
      started_at: "2026-07-15T00:00:00Z",
      completed_at: "2026-07-15T00:00:01Z",
      error: "stale",
    }]);
    resolveOldEvents([]);

    await waitFor(() => expect(screen.queryByText("job-stale")).not.toBeInTheDocument());
    expect(screen.getByText("job-other")).toBeInTheDocument();
  });

  it("clears project A logs immediately while project B is still loading", async () => {
    let resolveOtherJobs!: (value: unknown[]) => void;
    const otherJobs = new Promise<unknown[]>((resolve) => { resolveOtherJobs = resolve; });
    mocks.invoke.mockImplementation(async (command: string, args?: { cwd?: string }) => {
      if (command === "read_project_memory") {
        return { path: `${args?.cwd}/.codefactory/memory.md`, content: "", exists: false };
      }
      if (command === "get_effective_user_preference") return null;
      if (args?.cwd === "/other" && command === "list_evolution_jobs") return otherJobs;
      if (command === "list_evolution_jobs") return [{
        id: "job-project-a", cwd: "/proj", trigger: "cross_session", status: "succeeded",
        input_session_count: 2, input_trace_count: 8, candidate_count: 1,
        started_at: "2026-07-15T00:00:00Z", completed_at: "2026-07-15T00:00:01Z", error: null,
      }];
      if (command === "list_evolution_job_events") return [];
      return [];
    });
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/proj" />);
    await userEvent.click(await screen.findByRole("tab", { name: "作业与日志" }));
    expect(await screen.findByText("job-project-a")).toBeInTheDocument();

    await userEvent.selectOptions(screen.getByLabelText("项目范围"), "/other");

    await waitFor(() => expect(screen.queryByText("job-project-a")).not.toBeInTheDocument());
    expect(screen.getByText("正在加载作业")).toBeInTheDocument();
    await act(async () => resolveOtherJobs([]));
  });

  it("rejects an invalid deep-link scope before loading workbench data", async () => {
    render(<EvolutionWorkbenchPage onBack={() => {}} initialCwd="/quick-scratch" />);

    await waitFor(() => expect(mocks.load).toHaveBeenCalledWith("/proj"));
    expect(mocks.load).not.toHaveBeenCalledWith("/quick-scratch");
    expect(mocks.invoke).not.toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({ cwd: "/quick-scratch" }),
    );
  });
});
