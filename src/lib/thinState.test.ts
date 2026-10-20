import { describe, expect, it } from "vitest";
import { projectThinState, thinStateCopy } from "./thinState";

describe("thin 四态界面投影", () => {
  it("四态各自映射到一句人话，且不读 legacy 失败码", () => {
    const cases = [
      ["thin_running", "正在做"],
      ["thin_waiting_wake", "等唤醒"],
      ["thin_waiting_user", "需要你决定"],
      ["thin_ended", "已结束"],
    ] as const;
    for (const [kind, expected] of cases) {
      const projection = projectThinState({ kind, label: null, waitingReason: null });
      expect(projection.isThin).toBe(true);
      expect(thinStateCopy(projection)).toBe(expected);
    }
  });

  it("等待唤醒显示等待对象，等待用户显示问题，结束显示原因", () => {
    expect(
      thinStateCopy(
        projectThinState({ kind: "thin_waiting_wake", label: "等唤醒：PR #774 合并" }),
      ),
    ).toBe("等唤醒：PR #774 合并");
    expect(
      thinStateCopy(
        projectThinState({ kind: "thin_waiting_user", waitingReason: "是否继续等 CI" }),
      ),
    ).toBe("需要你决定：是否继续等 CI");
    expect(thinStateCopy(projectThinState({ kind: "thin_ended", label: "任务已结束" }))).toBe(
      "已结束：任务已结束",
    );
  });

  it("legacy 会话保持原投影，不被误标成 thin", () => {
    const projection = projectThinState({ kind: "technical_recovery_exhausted", label: "失败" });
    expect(projection.isThin).toBe(false);
    expect(thinStateCopy(projection)).toBeNull();
  });
});
