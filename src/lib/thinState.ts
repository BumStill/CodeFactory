/**
 * R6：thin 会话的界面投影只认四态，不从顶层失败码推。
 *
 * 后端把权威四态写在 `recent_activity_kind`（thin_running / thin_waiting_wake /
 * thin_waiting_user / thin_ended）；`recent_activity_label` 是给人看的一句话，
 * waiting_reason 是问题或等待对象。legacy 的 objective_status 只留给旧会话。
 */
export type ThinState = "running" | "waiting_wake" | "waiting_user" | "ended";

export interface ThinProjection {
  state: ThinState;
  label: string | null;
  question: string | null;
  isThin: boolean;
}

const KIND_TO_STATE: Record<string, ThinState> = {
  thin_running: "running",
  thin_waiting_wake: "waiting_wake",
  thin_waiting_user: "waiting_user",
  thin_ended: "ended",
};

export function projectThinState(activity: {
  kind?: string | null;
  label?: string | null;
  waitingReason?: string | null;
}): ThinProjection {
  const state = activity.kind ? KIND_TO_STATE[activity.kind] : undefined;
  if (!state) {
    return { state: "running", label: activity.label ?? null, question: null, isThin: false };
  }
  return {
    state,
    label: activity.label ?? null,
    question: state === "waiting_user" ? activity.waitingReason ?? activity.label ?? null : null,
    isThin: true,
  };
}

function prefixed(text: string | null, prefix: string): string {
  if (!text) return prefix;
  return text.startsWith(prefix) ? text : `${prefix}：${text}`;
}

/** 卡片上的四句人话；不再暴露 failure_code 之类的内部状态。 */
export function thinStateCopy(projection: ThinProjection): string | null {
  if (!projection.isThin) return null;
  switch (projection.state) {
    case "running":
      return "正在做";
    case "waiting_wake":
      return prefixed(projection.label, "等唤醒");
    case "waiting_user":
      return prefixed(projection.question, "需要你决定");
    case "ended":
      return projection.label ? `已结束：${projection.label}` : "已结束";
  }
}
