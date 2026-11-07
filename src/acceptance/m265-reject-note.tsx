// SPDX-License-Identifier: Apache-2.0
import React from "react";
import { createRoot } from "react-dom/client";
import "../styles/globals.css";
import { PermissionDialog } from "../components/PermissionDialog";

// M265 acceptance surface: the approval card must let the user type one sentence
// while refusing, and that sentence must arrive on the deny path (not as a
// separate message). Nothing here decides permissions — it only records what the
// card handed back.
function App() {
  const [outcome, setOutcome] = React.useState("waiting_user");
  const [note, setNote] = React.useState("");
  return (
    <main aria-label="M265 reject with note" className="min-h-screen bg-bg text-gray-200">
      <output data-testid="outcome">{outcome}</output>
      <output data-testid="note">{note}</output>
      <PermissionDialog
        trusted
        request={{
          intentId: "synthetic-m265",
          toolCallId: "synthetic-reset",
          toolName: "bash",
          args: { command: "git reset --hard" },
        }}
        onAllow={() => setOutcome("allowed")}
        onDeny={(submittedNote) => {
          setNote(submittedNote ?? "");
          setOutcome(submittedNote ? "denied_with_note" : "denied_without_note");
        }}
        onAllowFullAccess={() => setOutcome("allowed_full")}
      />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
