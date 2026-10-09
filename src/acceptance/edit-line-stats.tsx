// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for M36. jsdom does not lay out CSS, so this
// mounts the production ToolCallCard rows and the production
// TurnResultSnapshot with the production evidence summarizer, and lets a real
// layout engine decide whether "+X −Y" is legible (green/red, both themes),
// whether the old "985b → 4133b" summary is gone, and whether the turn total
// line stays inside the card.

import { useState } from "react";
import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { ToolCallCard } from "../components/ToolCallCard";
import { summarizeTurnEvidence, TurnResultSnapshot } from "../components/TurnResultSnapshot";
import type { TurnPlan } from "../lib/chatPlan";
import type { ToolCallState } from "../stores/chatEvents";

const editOneLine: ToolCallState = {
  id: "edit-one",
  name: "edit_file",
  args: JSON.stringify({
    path: "src/lib/session-title.ts",
    old_string: "const a = 1;\nconst b = 2;\nconst c = 3;",
    new_string: "const a = 1;\nconst b = 22;\nconst c = 3;",
  }),
  status: "done",
  result: "ok",
};

const editAdds: ToolCallState = {
  id: "edit-adds",
  name: "edit_file",
  args: JSON.stringify({
    path: "src/lib/chat-plan.ts",
    old_string: "export function start() {}",
    new_string: "export function start() {}\nexport function step() {}\nexport function finish() {}\nexport function stop() {}",
  }),
  status: "done",
  result: "ok",
};

const editRemoves: ToolCallState = {
  id: "edit-removes",
  name: "edit_file",
  args: JSON.stringify({
    path: "src/lib/dead-code.ts",
    old_string: "keep()\ngoneOne()\ngoneTwo()\nkeepTail()",
    new_string: "keep()\nkeepTail()",
  }),
  status: "done",
  result: "ok",
};

const writeNewFile: ToolCallState = {
  id: "write-new",
  name: "write_file",
  args: JSON.stringify({
    path: "docs/line-stats-notes.md",
    // Four lines; the write result below reports the matching "+1,4" hunk.
    content: "# 行数统计\n新增功能说明\n使用方法\n结束\n",
  }),
  status: "done",
  result: "Written 42 bytes to docs/line-stats-notes.md\n\n```diff\n--- a/docs/line-stats-notes.md\n+++ b/docs/line-stats-notes.md\n@@ -0,0 +1,4 @@\n+# 行数统计\n```",
};

const deniedEdit: ToolCallState = {
  id: "edit-denied",
  name: "edit_file",
  args: JSON.stringify({
    path: "src/lib/denied.ts",
    old_string: "a\nb\nc\nd\ne",
    new_string: "a",
  }),
  status: "denied",
  isError: true,
  result: "用户拒绝了这次编辑",
};

/** Failed/denied calls stay in the transcript but never enter the total. */
const turnCalls: ToolCallState[] = [
  editOneLine,
  editAdds,
  editRemoves,
  writeNewFile,
  deniedEdit,
];

const plan: TurnPlan = {
  rootTurnId: "root-m36",
  revision: 1,
  explanation: null,
  waitingReason: null,
  changeReason: null,
  waitingHistory: [],
  createdAt: 1,
  steps: [{ id: "edit", title: "按行数展示改动", kind: "implementation", status: "completed" }],
};

const evidence = summarizeTurnEvidence(turnCalls);

function App() {
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const toggle = () => {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.setAttribute("data-theme", next);
    setTheme(next);
  };

  return (
    <main
      aria-label="Edit line stats acceptance"
      className="min-h-screen bg-surface-1 p-6 text-gray-200"
    >
      <button
        type="button"
        onClick={toggle}
        className="mb-4 rounded-lg border border-border px-3 py-1.5 text-note"
      >
        切换主题 · {theme}
      </button>

      <section aria-label="Edit rows" className="max-w-[72ch] space-y-1">
        {turnCalls.map((call) => <ToolCallCard key={call.id} tc={call} />)}
      </section>

      <section aria-label="Turn total" className="mt-6 max-w-[72ch]">
        <TurnResultSnapshot
          plan={plan}
          evidence={evidence}
          objectiveStatus="completed"
          durationMs={180_000}
        />
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
