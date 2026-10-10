// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the M37 progress bar. It mounts the
// production TurnProgress in the four states the task names — a plan the agent
// never tracked, a tracked plan, the completion gate rerunning its checks, and a
// wait the user really has to act on — so a real layout engine decides whether
// the bar shows real progress, keeps the background loop off screen, and still
// warns when it must.

import { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { TurnProgress } from "../components/TurnProgress";
import type { TurnPlan, TurnTimingProfile } from "../lib/chatPlan";

const timing: TurnTimingProfile = {
  phases: {
    implementation: { sampleCount: 6, p25Ms: 180_000, p75Ms: 420_000 },
    verification: { sampleCount: 5, p25Ms: 120_000, p75Ms: 240_000 },
  },
  build: { sampleCount: 4, p25Ms: 90_000, p75Ms: 180_000 },
  externalJob: null,
};

/** The 2026-10-09 10:21 shape: five steps, none of them ever checked off. */
const untrackedPlan: TurnPlan = {
  rootTurnId: "root-untracked",
  revision: 1,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  waitingHistory: [],
  createdAt: 1,
  steps: Array.from({ length: 5 }, (_, index) => ({
    id: `step-${index + 1}`,
    title: `步骤 ${index + 1}`,
    kind: "implementation" as const,
    status: "pending" as const,
  })),
};

const trackedPlan: TurnPlan = {
  rootTurnId: "root-tracked",
  revision: 3,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  waitingHistory: [],
  createdAt: 1,
  steps: [
    { id: "inspect", title: "确认顶部进度条现状", kind: "analysis", status: "completed" },
    { id: "implement", title: "只按已跟踪步骤显示进度", kind: "implementation", status: "completed" },
    { id: "verify", title: "真实浏览器验收", kind: "verification", status: "in_progress" },
    { id: "deliver", title: "开 PR", kind: "delivery", status: "pending" },
  ],
};

const authorizingPlan: TurnPlan = {
  ...trackedPlan,
  rootTurnId: "root-authorizing",
  waitingReason: "authorization_required",
};

function App() {
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const toggle = () => {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.setAttribute("data-theme", next);
    setTheme(next);
  };

  return (
    <main
      aria-label="Turn progress acceptance"
      className="min-h-screen bg-surface-1 p-6 text-gray-200"
    >
      <button
        type="button"
        onClick={toggle}
        className="mb-4 rounded-lg border border-border px-3 py-1.5 text-note"
      >
        切换主题 · {theme}
      </button>

      {/* State (a): the agent never checked off a step. No count, no percent. */}
      <section aria-label="Plan not tracked">
        <TurnProgress
          plan={untrackedPlan}
          timingProfile={timing}
          externalJobs={[]}
          elapsedMs={449_000}
        />
      </section>

      {/* State (b): steps are tracked, so the real count may be shown. */}
      <section aria-label="Plan tracked">
        <TurnProgress
          plan={trackedPlan}
          timingProfile={timing}
          externalJobs={[]}
          elapsedMs={90_000}
        />
      </section>

      {/* State (c): the completion gate rerunning checks is normal background
          work — neutral tone, plain words, no jargon. */}
      <section aria-label="Completion gate rerunning checks">
        <TurnProgress
          plan={{ ...trackedPlan, rootTurnId: "root-gate", waitingReason: null }}
          timingProfile={timing}
          externalJobs={[]}
          elapsedMs={132_000}
          activityLabel="正在补跑检查"
          activityWaitingReason="验证证据不足"
        />
      </section>

      {/* State (d): a wait the user really has to clear keeps its warning. */}
      <section aria-label="Needs authorization">
        <TurnProgress
          plan={authorizingPlan}
          timingProfile={timing}
          externalJobs={[]}
          elapsedMs={210_000}
        />
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
