// SPDX-License-Identifier: Apache-2.0
import { useRef, useState, useEffect, ChangeEvent, KeyboardEvent, ClipboardEvent, DragEvent } from "react";
import { recallHistory, pushHistory } from "./messageHistory";
import { Send, Square, Paperclip, X, Loader2 } from "lucide-react";
import {
  filterSlashCommandSuggestions,
  parseSlashCommand,
  type ParsedSlashCommand,
} from "../stores/slashCommands";
import { invoke } from "../lib/tauri";

interface SkillSlashCommand {
  name: string;
  description: string;
  template: string;
}

/** A file the user pasted, dropped, or picked — already persisted under
 *  `<cwd>/.codefactory/attachments/`. Images embed as markdown image links
 *  (the agent reads them as vision blocks); documents embed as a labelled
 *  path the agent can hand to read_pptx / read_file. */
interface AttachmentChip {
  id: string;
  path: string;
  name: string;
  sizeBytes: number;
}

const IMAGE_EXTS = ["png", "jpg", "jpeg", "gif", "webp"];
const DOC_EXTS = ["pptx", "docx", "pdf"];
const ACCEPT_ATTR = [...IMAGE_EXTS, ...DOC_EXTS].map((e) => `.${e}`).join(",");
const MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024;

function extOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i >= 0 ? name.slice(i + 1).toLowerCase() : "";
}
function isImageAttachment(name: string): boolean {
  return IMAGE_EXTS.includes(extOf(name));
}

interface Props {
  onSend: (text: string) => void;
  onCommand?: (command: ParsedSlashCommand) => void | Promise<void>;
  onCancel: () => void;
  streaming: boolean;
  disabled: boolean;
  /** When set, this text will be appended to the current input value. */
  pendingInsert?: string;
  /** Called after pendingInsert has been consumed. */
  onInsertConsumed?: () => void;
  /** Extra slash commands from enabled skills */
  skillSlashCommands?: SkillSlashCommand[];
  /** cwd of the active session — required for attachment saves to land
   *  in the right project's .codefactory/attachments dir. Without it,
   *  paste/drop are silently ignored. */
  cwd?: string | null;
}

export function MessageInput({ onSend, onCommand, onCancel, streaming, disabled, pendingInsert, onInsertConsumed, skillSlashCommands = [], cwd }: Props) {
  const [value, setValue] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);
  const [attachments, setAttachments] = useState<AttachmentChip[]>([]);
  const [uploading, setUploading] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [attachError, setAttachError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  // Shell-style input history: sent messages this session + where we are in it.
  const [history, setHistory] = useState<string[]>([]);
  const [histPos, setHistPos] = useState(0);
  const draftRef = useRef("");

  useEffect(() => {
    if (!pendingInsert) return;
    setValue((prev) => (prev ? `${prev} ${pendingInsert}` : pendingInsert));
    setHistPos(0); // an insert exits history-recall mode, like typing does
    onInsertConsumed?.();
    ref.current?.focus();
  }, [pendingInsert]);

  // ── Attachment plumbing ──────────────────────────────────────────────
  //
  // Both paste and drag-drop funnel through saveAttachmentFile: turn a
  // File into base64 → invoke the Tauri save_chat_attachment command →
  // get back the on-disk path → add a chip. The actual markdown link
  // is appended at SEND time, not now, so chips can be removed without
  // hunting through the textarea content.
  const saveAttachmentFile = async (file: File): Promise<void> => {
    if (!cwd) {
      setAttachError("打开一个项目后才能附加文件");
      return;
    }
    setUploading(true);
    setAttachError(null);
    try {
      const buf = await file.arrayBuffer();
      // Browser-safe base64 of binary data.
      let binary = "";
      const bytes = new Uint8Array(buf);
      const CHUNK = 0x8000;
      for (let i = 0; i < bytes.length; i += CHUNK) {
        binary += String.fromCharCode.apply(null, Array.from(bytes.subarray(i, i + CHUNK)));
      }
      const b64 = btoa(binary);
      const saved = await invoke<AttachmentChip & { size_bytes: number }>(
        "save_chat_attachment",
        { cwd, filename: file.name || "pasted.png", dataBase64: b64 },
      );
      // Display the original filename (the on-disk name is a uuid); the
      // original name's extension is what tells images from documents.
      setAttachments((prev) => [
        ...prev,
        { id: crypto.randomUUID(), path: saved.path, name: file.name || saved.name, sizeBytes: saved.size_bytes },
      ]);
    } catch (e) {
      setAttachError(String(e));
    } finally {
      setUploading(false);
    }
  };

  const onPaste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const item of Array.from(items)) {
      if (item.kind === "file" && item.type.startsWith("image/")) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) void saveAttachmentFile(file);
        return;
      }
    }
  };

  const enqueueFiles = (files: File[]) => {
    for (const f of files) {
      // Cap per-file to avoid choking the IPC bridge on huge drops.
      if (f.size > MAX_ATTACHMENT_BYTES) {
        setAttachError(`${f.name} 超过 25MB 上限，已跳过`);
        continue;
      }
      void saveAttachmentFile(f);
    }
  };

  const onDrop = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    setDragOver(false);
    enqueueFiles(Array.from(e.dataTransfer?.files ?? []));
  };

  const onPickFiles = (e: ChangeEvent<HTMLInputElement>) => {
    enqueueFiles(Array.from(e.target.files ?? []));
    // Reset so picking the same file again still fires onChange.
    e.target.value = "";
  };

  const onDragOver = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    if (!dragOver) setDragOver(true);
  };
  const onDragLeave = () => setDragOver(false);

  const removeAttachment = (id: string) => {
    setAttachments((prev) => prev.filter((a) => a.id !== id));
  };

  // Merge builtin + skill slash command suggestions
  const builtinSuggestions = filterSlashCommandSuggestions(value);
  const skillSuggestions = value.startsWith("/") && !value.includes(" ")
    ? skillSlashCommands
        .filter((c) => `/${c.name}`.startsWith(value.toLowerCase()))
        .map((c) => ({ name: c.name, usage: `/${c.name}`, description: c.description, argumentHint: "{input}" as string | undefined }))
    : [];
  const suggestions = [
    ...builtinSuggestions,
    ...skillSuggestions.filter((s) => !builtinSuggestions.some((b) => b.name === s.name)),
  ];
  const commandCandidate = parseSlashCommand(value.trim());

  const submit = () => {
    const text = value.trim();
    // Allow submission with attachments but no text — the markdown links
    // appended below count as content for the model.
    if (!text && attachments.length === 0) return;
    setHistPos(0); // leave history-navigation mode on any submit
    const command = parseSlashCommand(text);
    // Slash commands run synchronously and are never queued — they would
    // be confusing as deferred work (`/cwd` two stream-finishes later).
    if (command) {
      if (!onCommand) return;
      setValue("");
      setAttachments([]);
      ref.current!.style.height = "auto";
      void onCommand(command);
      return;
    }
    if (disabled) return;
    // Record real (non-command) sends for ↑/↓ recall — slash commands run
    // locally and shouldn't pollute message history.
    if (text) setHistory((h) => pushHistory(h, text));
    // Append attachments at send time so the user can freely remove chips
    // before send without text-editing the textarea. Images become vision
    // markdown links; documents become a labelled path the agent reads with
    // read_pptx (preserving-edit) or read_file (plain-text extraction).
    let outgoing = text;
    if (attachments.length > 0) {
      const blocks: string[] = [];
      const images = attachments.filter((a) => isImageAttachment(a.name));
      const docs = attachments.filter((a) => !isImageAttachment(a.name));
      for (const a of images) blocks.push(`![${a.name}](file://${a.path})`);
      if (docs.length > 0) {
        const lines = docs.map((a) => `- ${a.name} — 本地路径: ${a.path}`).join("\n");
        blocks.push(
          `已上传以下文件（.pptx 用 read_pptx 读结构后：edit_pptx 原地增强内容、format_pptx 统一美化排版；要总结/演讲稿就读取后在聊天框回答或 write_docx 生成。.docx/.pdf 用 read_file 提取文本）：\n${lines}`,
        );
      }
      const appendix = blocks.join("\n\n");
      outgoing = text ? `${text}\n\n${appendix}` : appendix;
    }
    setValue("");
    setAttachments([]);
    setAttachError(null);
    ref.current!.style.height = "auto";
    onSend(outgoing);
  };

  const moveCursorToEnd = () => {
    requestAnimationFrame(() => {
      const el = ref.current;
      if (el) {
        el.selectionStart = el.selectionEnd = el.value.length;
        autoResize();
      }
    });
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
      return;
    }
    // Shell-style history recall. ↑ enters history only when the caret is at
    // the very start (so multi-line editing isn't hijacked); once navigating,
    // ↑/↓ step through it. ↓ walks back out to the live draft.
    if (e.key === "ArrowUp" && history.length > 0) {
      const el = ref.current;
      const atStart = !!el && el.selectionStart === 0 && el.selectionEnd === 0;
      if (histPos > 0 || atStart) {
        e.preventDefault();
        if (histPos === 0) draftRef.current = value;
        const { value: v, pos } = recallHistory(history, histPos, "up", draftRef.current);
        setValue(v);
        setHistPos(pos);
        moveCursorToEnd();
      }
      return;
    }
    if (e.key === "ArrowDown" && histPos > 0) {
      e.preventDefault();
      const { value: v, pos } = recallHistory(history, histPos, "down", draftRef.current);
      setValue(v);
      setHistPos(pos);
      moveCursorToEnd();
    }
  };

  const autoResize = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 200) + "px";
  };

  // canSubmit recomputes with attachments too — a chip-only message is valid.
  const hasContent = Boolean(value.trim()) || attachments.length > 0;
  const submitReady = hasContent && (!disabled || Boolean(commandCandidate));

  return (
    <div
      className={`border-t border-border bg-surface-1 px-4 py-3 transition-colors ${
        dragOver ? "bg-accent/5" : ""
      }`}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      {suggestions.length > 0 && !streaming && (
        <div className="mb-2 max-h-48 overflow-auto rounded-lg border border-border bg-surface-2 shadow-xl">
          {suggestions.map((command) => (
            <button
              key={command.name}
              type="button"
              onClick={() => {
                setValue(command.argumentHint ? `/${command.name} ` : command.usage);
                ref.current?.focus();
              }}
              className="grid w-full grid-cols-[7rem_minmax(0,1fr)] gap-3 px-3 py-2 text-left text-xs hover:bg-surface-3"
            >
              <span className="font-mono text-accent">{command.usage}</span>
              <span className="truncate text-gray-500">{command.description}</span>
            </button>
          ))}
        </div>
      )}
      {/* Attachment chips — shown above the textarea, removable per chip */}
      {(attachments.length > 0 || uploading || attachError) && (
        <div className="mb-2 flex flex-wrap gap-1.5 items-center">
          {attachments.map((a) => (
            <span
              key={a.id}
              className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-surface-3 border border-border text-[11px] text-gray-300"
              title={a.path}
            >
              <Paperclip size={10} className="text-accent" />
              <span className="truncate max-w-[160px]">{a.name}</span>
              <span className="text-gray-600 text-[10px]">
                {(a.sizeBytes / 1024).toFixed(0)}KB
              </span>
              <button
                onClick={() => removeAttachment(a.id)}
                className="text-gray-500 hover:text-red-400"
                title="移除"
              >
                <X size={10} />
              </button>
            </span>
          ))}
          {uploading && (
            <span className="inline-flex items-center gap-1 text-[11px] text-gray-500">
              <Loader2 size={10} className="animate-spin" /> 保存中…
            </span>
          )}
          {attachError && (
            <span className="text-[11px] text-red-700 dark:text-red-300">{attachError}</span>
          )}
        </div>
      )}
      <div className="flex items-end gap-2 rounded-xl border border-border bg-surface-2 px-3 py-2 focus-within:border-accent/50 transition-colors">
        <input
          ref={fileInputRef}
          type="file"
          multiple
          accept={ACCEPT_ATTR}
          className="hidden"
          onChange={onPickFiles}
        />
        <button
          type="button"
          onClick={() => fileInputRef.current?.click()}
          disabled={!cwd || uploading}
          className="shrink-0 rounded-lg p-1.5 transition-colors enabled:hover:bg-surface-4 text-gray-500 disabled:opacity-30"
          title={cwd ? "附加文件（图片 / pptx / docx / pdf）" : "打开项目后可附加文件"}
        >
          <Paperclip size={16} />
        </button>
        <textarea
          ref={ref}
          value={value}
          onChange={(e) => { setValue(e.target.value); setHistPos(0); autoResize(); }}
          onKeyDown={onKey}
          onPaste={onPaste}
          rows={1}
          placeholder={
            dragOver
              ? "松开以附加文件"
              : disabled
              ? "Message or /cwd <path>"
              : "Message · 粘贴/拖拽/回形针附加文件（图片 · pptx · docx · pdf）"
          }
          className="flex-1 resize-none bg-transparent text-sm text-gray-200 placeholder-gray-600 outline-none min-h-[24px] max-h-[200px] leading-6 disabled:opacity-40"
        />
        {/* Two buttons during streaming: queue-send (default, primary)
            and cancel-stream (secondary, square icon). Outside streaming
            it's just the regular send. */}
        {streaming && submitReady && (
          <button
            onClick={submit}
            disabled={!submitReady}
            className="shrink-0 rounded-lg p-1.5 transition-colors enabled:hover:bg-surface-4 text-accent"
            title="排队（流式结束后自动发送）"
          >
            <Send size={16} />
          </button>
        )}
        <button
          onClick={streaming ? onCancel : submit}
          disabled={!streaming && !submitReady}
          className="shrink-0 rounded-lg p-1.5 transition-colors disabled:opacity-30
            enabled:hover:bg-surface-4 text-accent disabled:text-gray-600"
          title={streaming ? "Cancel stream" : "Send (Enter)"}
        >
          {streaming ? <Square size={16} /> : <Send size={16} />}
        </button>
      </div>
      <div className="mt-1 text-xs text-gray-700 text-right select-none">
        {streaming ? "Enter 排队 · Shift+Enter 换行" : "Enter 发送 · Shift+Enter 换行"}
      </div>
    </div>
  );
}
