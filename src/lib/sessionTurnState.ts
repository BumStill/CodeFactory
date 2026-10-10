// SPDX-License-Identifier: Apache-2.0
//
// CF-HDE-R7 / R9 / R11 —— 会话"真实状态"的单一来源。
//
// 第二阶段的三个真机缺口（M56 / M55 / M53）根子都在同一件事：界面上有好几处
// 各自猜测"这个会话现在到底在不在跑"。侧栏转圈读 `runtime.streaming`，顶部提示
// 读消息里的 turnActivity，派单入口的 `send` 又读 `runtime.streaming` —— 三份
// 猜测，于是三份不同的答案：旧的回合结算之后 `streaming` 残留为 true，侧栏一直
// 转圈、objective 一直显示"等待自动重试"、新消息被当成"运行中"塞进一个永远不
// 会排空的队列（M56）。
//
// 这个模块只做一件事：把权威证据合成为一个结论。权威证据是——
//   * 服务器给的 `session.is_running`（一次运行是否真的拥有这个会话）；
//   * 本地消息投影里的 root turn 归属与 turnActivity（`turnOwnership`）；
//   * 是否有未决的权限询问（组件层传入）。
// 结果被三处共用：派单 `status`（R9）、顶部提示与侧栏转圈（R11）、以及 `send`
// 判断"该立即开新一轮还是插话/排队"（R7）。三处读同一个函数，就不可能各说各话。

import { currentTurnOwnership, systemOwnsObjective } from "./turnOwnership";
import type { UIMessage } from "../stores/chatEvents";

/** 与 `chatEvents` 的 objectiveStatus 对齐，另加本地空闲态。 */
export type SessionObjectiveState =
  | "active"
  | "waiting_system"
  | "waiting_core_input"
  | "waiting_authorization"
  | "waiting_business_decision"
  | "completed"
  | "failed"
  | "cancelled"
  | "legacy_orphan"
  | "idle";

export interface SessionTurnStateInput {
  /** 本地 runtime 的 streaming 投影。可能是残留值，不能单独采信。 */
  streaming: boolean;
  /** 服务器权威标志：一次运行当前是否拥有这个会话。 */
  sessionIsRunning?: boolean | null;
  /** 本地消息投影（用于 root turn 归属与 turnActivity）。 */
  messages: UIMessage[];
  /** 是否有未决的权限询问。 */
  waitingPermission?: boolean;
  /** 最近一次模型调用的上下文大小（token）。 */
  contextTokens?: number | null;
  /** 交付记录里的 PR 号。 */
  prNumber?: number | null;
}

export interface SessionTurnState {
  /** 此刻是否真的有一轮在执行。残留的旧回合不算。 */
  running: boolean;
  /** 与真实执行一致的 objective 状态。 */
  objectiveState: SessionObjectiveState;
  /** 是否在等待一次权限决策。 */
  waitingPermission: boolean;
  /** 最近一条非空助理回复的摘要，没有则 null。 */
  latestReply: string | null;
  /** 最近一次模型调用的上下文大小（token），未知则 null。 */
  contextTokens: number | null;
  /** 交付记录里的 PR 号，未知则 null。 */
  prNumber: number | null;
}

/** 摘要上限：够编排方判断"该追加还是另开"，又不把整段回复搬进日志。 */
export const LATEST_REPLY_MAX_CHARS = 280;

/** 最近一条冻结的 objective 投影（若有）。 */
function lastObjectiveStatus(messages: UIMessage[]): SessionObjectiveState | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const status = messages[index].turnActivity?.objectiveStatus;
    if (status) return status as SessionObjectiveState;
  }
  return null;
}

/** 把一段回复压成单行摘要。 */
export function summarizeReply(content: string, maxChars = LATEST_REPLY_MAX_CHARS): string | null {
  const collapsed = content.replace(/\s+/g, " ").trim();
  if (!collapsed) return null;
  if (collapsed.length <= maxChars) return collapsed;
  return `${collapsed.slice(0, maxChars)}…`;
}

function latestAssistantReply(messages: UIMessage[]): string | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role !== "assistant") continue;
    const summary = summarizeReply(message.content);
    if (summary) return summary;
  }
  return null;
}

/**
 * 合成一个会话的真实状态。
 *
 * 优先级反映了"什么消息更重要"：一个未决的权限询问压过一切（用户不动就没有
 * 下一步）；一个真正在执行的回血压过任何陈旧的等待投影（M53：在调用模型就不能
 * 显示等待重试）；只有确认没有任何东西在执行时，"等待系统"或某个终止态才是
 * 可信的。
 */
export function sessionTurnState(input: SessionTurnStateInput): SessionTurnState {
  const messages = input.messages ?? [];
  const ownership = currentTurnOwnership(messages);
  const frozen = lastObjectiveStatus(messages);
  const serverRunning = input.sessionIsRunning === true;
  // 只有在"这一轮还没被释放"时，本地 streaming 才说明它在跑；结算后的残留
  // 不算（M56）。服务器权威标志永远是证据。
  const running = serverRunning || (input.streaming && !ownership.released);
  const waitingPermission = input.waitingPermission === true;

  let objectiveState: SessionObjectiveState;
  if (waitingPermission) {
    objectiveState = "waiting_authorization";
  } else if (running) {
    objectiveState = "active";
  } else if (!ownership.released && ownership.systemHeld) {
    // 系统仍持有这一轮，但此刻没有在执行：这才是真正该显示"等待系统"的时候。
    objectiveState =
      frozen && systemOwnsObjective(frozen) ? frozen : "waiting_system";
  } else if (frozen) {
    objectiveState = frozen;
  } else {
    objectiveState = "idle";
  }

  return {
    running,
    objectiveState,
    waitingPermission,
    latestReply: latestAssistantReply(messages),
    contextTokens: input.contextTokens ?? null,
    prNumber: input.prNumber ?? null,
  };
}
