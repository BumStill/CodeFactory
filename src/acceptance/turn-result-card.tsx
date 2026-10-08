// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the M31 result card. It mounts the
// production TurnResultSnapshot (with the production evidence summarizer) in
// the three states the task names — failed with no PR but files changed,
// completed with a PR, and waiting — so a real layout engine decides whether
// the card says what happened, where the work is and what comes next.

import { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import {
  summarizeTurnEvidence,
  TurnResultSnapshot,
} from "../components/TurnResultSnapshot";
import type { TurnPlan } from "../lib/chatPlan";
import type { ToolCallState } from "../stores/chatEvents";

function editedFiles(count: number): ToolCallState[] {
  return Array.from({ length: count }, (_, index): ToolCallState => ({
    id: `edit-${index}`,
    name: "edit_file",
    args: JSON.stringify({ path: `src/changed-${index + 1}.ts` }),
    status: "done",
    result: "ok",
  }));
}

const untrackedPlan: TurnPlan = {
  rootTurnId: "root-failed",
  revision: 1,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  waitingHistory: [],
  createdAt: 1,
  // The agent never updated its steps: the old card rendered a misleading 0/5.
  steps: Array.from({ length: 5 }, (_, index) => ({
    id: `step-${index + 1}`,
    title: `步骤 ${index + 1}`,
    kind: "implementation" as const,
    status: "pending" as const,
  })),
};

const failedEvidence = summarizeTurnEvidence(editedFiles(6));

const completedEvidence = summarizeTurnEvidence([
  ...editedFiles(2),
  {
    id: "deliver",
    name: "deliver_changes",
    args: JSON.stringify({ title: "fix: thing" }),
    status: "done",
    result: "Delivered. PR opened: https://github.com/BumStill/CodeFactory/pull/568",
  },
]);

const waitingPlan: TurnPlan = {
  rootTurnId: "root-waiting",
  revision: 4,
  explanation: null,
  waitingReason: "等待 GitHub required checks",
  nextActionOwner: "external",
  changeReason: null,
  waitingHistory: ["等待 GitHub required checks"],
  createdAt: 1,
  steps: [
    { id: "inspect", title: "确认改动", kind: "analysis", status: "completed" },
    { id: "deliver", title: "等待 CI", kind: "external_job", status: "in_progress" },
  ],
};

const waitingEvidence = summarizeTurnEvidence(editedFiles(3));

function App() {
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const toggle = () => {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.setAttribute("data-theme", next);
    setTheme(next);
  };

  return (
    <main
      aria-label="Turn result card acceptance"
      className="min-h-screen bg-surface-1 p-6 text-gray-200"
    >
      <button
        type="button"
        onClick={toggle}
        className="mb-4 rounded-lg border border-border px-3 py-1.5 text-note"
      >
        切换主题 · {theme}
      </button>

      <section aria-label="Failed without a PR">
        <TurnResultSnapshot
          plan={untrackedPlan}
          evidence={failedEvidence}
          objectiveStatus="failed"
          turnBoundaryFailure
          durationMs={880_000}
        />
      </section>

      <section aria-label="Completed with a PR">
        <TurnResultSnapshot
          plan={{ ...untrackedPlan, rootTurnId: "root-done", steps: [], revision: 2 }}
          evidence={completedEvidence}
          objectiveStatus="completed"
          durationMs={120_000}
        />
      </section>

      <section aria-label="Waiting on CI">
        <TurnResultSnapshot
          plan={waitingPlan}
          evidence={waitingEvidence}
          durationMs={240_000}
        />
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
