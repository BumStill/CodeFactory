// SPDX-License-Identifier: Apache-2.0
import { useState } from "react";
import { createRoot } from "react-dom/client";
import "../styles/globals.css";
import { MessageInput } from "../components/MessageInput";
import { MessageList } from "../components/MessageList";
import type { UIMessage } from "../stores/chatEvents";
const png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
const calls: unknown[] = [];
Object.assign(window, { __m190Calls: calls, __TAURI_INTERNALS__: {
  invoke: async (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "save_session_attachment") return { path: `/synthetic/attachments/${args.filename}`, name: args.filename, size_bytes: 2097152 };
    if (command === "attachment_metadata") return { size_bytes: 2097152 };
    if (command === "paste_session_attachments") return [{ path: "/synthetic/attachments/native.pdf", name: "native.pdf", size_bytes: 2048 }];
    if (command === "open_chat_link" && String(args.target).includes("missing")) throw new Error("文件已删除：missing.pdf");
    return null;
  }, convertFileSrc: () => png, transformCallback: () => 1,
} });
function Acceptance() {
  const [messages, setMessages] = useState<UIMessage[]>(() => JSON.parse(localStorage.getItem("m190-history") ?? "[]"));
  return <main aria-label="M190 attachments" className="flex h-screen flex-col bg-surface-0 text-gray-200">
    <div className="min-h-0 flex-1"><MessageList messages={messages} streaming={false} cwd="/synthetic" conversationKey="m190" /></div>
    <MessageInput cwd="/synthetic" sessionId="m190" onSend={(content) => {
      const next: UIMessage[] = [...messages, { id: crypto.randomUUID(), role: "user", content, createdAt: Date.now() }];
      localStorage.setItem("m190-history", JSON.stringify(next)); setMessages(next);
    }} onCancel={() => {}} streaming={false} disabled={false} />
  </main>;
}
createRoot(document.getElementById("root")!).render(<Acceptance />);
