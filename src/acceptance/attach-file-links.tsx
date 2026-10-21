// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { createRoot } from "react-dom/client";
import "../styles/globals.css";
import { MessageInput } from "../components/MessageInput";
import { MessageList } from "../components/MessageList";
import { FileArtifactCard } from "../components/FileArtifactCard";
import type { UIMessage } from "../stores/chatEvents";
const calls: unknown[] = [];
Object.assign(window, { __m135Calls: calls, __TAURI_INTERNALS__: {
  invoke: async (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "save_chat_attachment") return { path: "/synthetic/app/attachments/draft/input.png", name: "input.png", size_bytes: 4 };
    if (command === "open_chat_link" && args.target === "missing.pdf") throw new Error("file not found: missing.pdf");
    return null;
  },
  convertFileSrc: () => "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==",
  transformCallback: () => 1,
}});
function Acceptance() {
  const [messages, setMessages] = useState<UIMessage[]>([{ id: "synthetic-message", role: "user", createdAt: 1, content: "[本地报告](file:///synthetic/report.pdf) [相对报告](report.pdf) [缺失文件](missing.pdf) [网站](https://example.com)" }]);
  return <main aria-label="Attachment file links acceptance" className="flex h-screen flex-col bg-surface-0 text-gray-200">
    <MessageList messages={messages} streaming={false} cwd={null} conversationKey="draft" />
    <FileArtifactCard path="report.pdf" />
    <MessageInput cwd={null} sessionId="draft" onSend={(content) => setMessages((old) => [...old, { id: "sent", role: "user", content, createdAt: 2 }])} onCancel={() => {}} streaming={false} disabled={false} />
  </main>;
}
createRoot(document.getElementById("root")!).render(<Acceptance />);
