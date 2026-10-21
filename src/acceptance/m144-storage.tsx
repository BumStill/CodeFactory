// SPDX-License-Identifier: Apache-2.0
import { createRoot } from "react-dom/client";
import { MessageList } from "../components/MessageList";
import { presentChatInvocationError } from "../stores/chatEvents";
import "../styles/globals.css";

const presentation = presentChatInvocationError("Database error: (code: 5) database is locked");
createRoot(document.getElementById("root")!).render(
  <main aria-label="M144 acceptance" style={{ height: "100vh" }}>
    <MessageList streaming={false} messages={[
      { id: "synthetic-input", role: "user", content: "继续 synthetic", createdAt: 1 },
      { id: "synthetic-result", role: "assistant", content: presentation.content, createdAt: 2 },
      { id: "synthetic-stop", role: "assistant", content: "已停止。已有内容和本地改动已保留。", createdAt: 3 },
    ]} />
  </main>,
);
