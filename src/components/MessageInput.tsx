// SPDX-License-Identifier: Apache-2.0
import { useRef, useState, useEffect, ChangeEvent, KeyboardEvent, ClipboardEvent, DragEvent, type ReactNode } from "react";
import { recallHistory, pushHistory } from "./messageHistory";
import { Send, Square, Paperclip, X, Loader2, Check } from "lucide-react";
import {
  filterSlashCommandSuggestions,
  parseSlashCommand,
  type ParsedSlashCommand,
} from "../stores/slashCommands";
import { invoke } from "../lib/tauri";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ImagePreview } from "./ImagePreview";
import { ComposerControlBar } from "./ComposerControlBar";

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
const DOC_EXTS = ["pptx", "docx", "pdf", "xlsx"];
const ACCEPT_ATTR = [...IMAGE_EXTS, ...DOC_EXTS].map((e) => `.${e}`).join(",");
const MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024;

function extOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i >= 0 ? name.slice(i + 1).toLowerCase() : "";
}
function isImageAttachment(name: string): boolean {
  return IMAGE_EXTS.includes(extOf(name));
}
function attachmentImageSrc(path: string): string {
  const localPath = path.startsWith("file://") ? path.slice("file://".length) : path;
  return convertFileSrc(localPath);
}

function attachmentImageMarkdown(name: string, path: string): string {
  const url = path.startsWith("file://") ? path : `file://${path}`;
  const destination = /\s/.test(url) ? `<${url}>` : url;
  return `![${name}](${destination})`;
}

interface Props {
  onSend: (text: string) => void;
  /** Route the primary input as an autonomous-run interjection instead of a chat turn. */
  onGuide?: (text: string) => Promise<void>;
  onCommand?: (command: ParsedSlashCommand) => void | Promise<void>;
  onCancel: () => void;
  streaming: boolean;
  /** True while an autonomous task run is active; Enter submits guidance for the next task. */
  guidanceActive?: boolean;
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
  /** Seed ↑/↓ recall with this session's already-sent user messages (oldest
   *  first). Combined with a per-session `key`, recall is scoped per session. */
  initialHistory?: string[];
  /** Controls for project/model/permission/context inside the input surface. */
  toolbar?: ReactNode;
}

/** How long after compositionend an Enter keydown is still treated as the
 *  candidate-commit key rather than a send (WebKit event ordering). Real
 *  human "commit then send" double-Enters measure well above this. */
const IME_COMMIT_GRACE_MS = 100;

export function MessageInput({ onSend, onGuide, onCommand, onCancel, streaming, guidanceActive = false, disabled, pendingInsert, onInsertConsumed, skillSlashCommands = [], cwd, initialHistory, toolbar }: Props) {
  const [value, setValue] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);
  const [attachments, setAttachments] = useState<AttachmentChip[]>([]);
  const [uploading, setUploading] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [attachError, setAttachError] = useState<string | null>(null);
  const [guidanceState, setGuidanceState] = useState<
    { kind: "idle" } | { kind: "pending" } | { kind: "success" } | { kind: "error"; message: string }
  >({ kind: "idle" });
  const fileInputRef = useRef<HTMLInputElement>(null);
  // Shell-style input history: sent messages this session + where we are in it.
  // Seeded from the session's prior user messages (the component is keyed by
  // session, so this re-seeds on switch).
  const [history, setHistory] = useState<string[]>(() => initialHistory ?? []);
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

  const submit = async ({ forceQueue = false }: { forceQueue?: boolean } = {}) => {
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
    if (guidanceActive && !forceQueue && text && onGuide) {
      setGuidanceState({ kind: "pending" });
      try {
        await onGuide(text);
        setValue("");
        setAttachments([]);
        setAttachError(null);
        if (ref.current) ref.current.style.height = "auto";
        setGuidanceState({ kind: "success" });
      } catch (error) {
        setGuidanceState({ kind: "error", message: String(error) });
      }
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
      for (const a of images) blocks.push(attachmentImageMarkdown(a.name, a.path));
      if (docs.length > 0) {
        const lines = docs.map((a) => `- ${a.name} — 本地路径: ${a.path}`).join("\n");
        blocks.push(
          `已上传以下文件（.pptx 用 read_pptx 读结构后：edit_pptx 原地增强内容、format_pptx 统一美化排版；.xlsx 用 read_xlsx 读成表格、edit_xlsx 把结果写回单元格（如逐行总结到某列）；要总结/演讲稿就读取后在聊天框回答或 write_docx 生成。.docx/.pdf 用 read_file 提取文本）：\n${lines}`,
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

  // ── IME composition tracking (see the Enter guard below) ──
  const composingRef = useRef(false);
  const compositionEndedAtRef = useRef(0);

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      // IME guard: Enter with the candidate list open COMMITS the
      // composition, it doesn't send. Chromium delivers that keydown while
      // still composing; WebKit (our runtime) fires compositionend FIRST and
      // then the same physical Enter as a plain keydown — only the short
      // window after compositionend tells it apart from a real send.
      if (
        composingRef.current ||
        e.nativeEvent.isComposing ||
        Date.now() - compositionEndedAtRef.current < IME_COMMIT_GRACE_MS
      ) {
        return;
      }
      e.preventDefault();
      // While a run is in flight the default is to steer it. ⌘/Ctrl+Enter is
      // the escape hatch for "this is a separate thing, do it after" — a
      // per-message modifier, not a mode the user has to manage.
      void submit({ forceQueue: e.metaKey || e.ctrlKey });
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
  const submitReady = guidanceActive
    ? Boolean(value.trim()) && guidanceState.kind !== "pending"
    : hasContent && (!disabled || Boolean(commandCandidate));

  return (
    <div
      className={`transition-colors ${
        dragOver ? "rounded-xl bg-status-progress-soft" : ""
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
      {/* Attachment previews — images show thumbnails; documents stay as chips. */}
      {(attachments.length > 0 || uploading || attachError) && (
        <div className="mb-2 flex flex-wrap items-end gap-2">
          {attachments.map((a) => (
            isImageAttachment(a.name) ? (
              <figure
                key={a.id}
                className="group relative overflow-hidden rounded-xl border border-border bg-surface-3 p-1 shadow-sm"
                title={a.path}
              >
                <ImagePreview
                  src={attachmentImageSrc(a.path)}
                  alt={a.name}
                  thumbnailClassName="block h-20 w-28 rounded-lg bg-surface-2 object-cover transition-opacity hover:opacity-90"
                  title={a.path}
                />
                <figcaption className="mt-1 flex max-w-28 items-center gap-1 text-[11px] text-gray-400">
                  <span className="truncate">{a.name}</span>
                  <span className="shrink-0 text-gray-600">{(a.sizeBytes / 1024).toFixed(0)}KB</span>
                </figcaption>
                <button
                  type="button"
                  onClick={() => removeAttachment(a.id)}
                  className="absolute right-0 top-0 flex h-11 w-11 items-center justify-center rounded-full bg-surface-0/80 text-gray-400 opacity-90 transition-colors hover:bg-status-danger-soft hover:text-status-danger lg:right-1 lg:top-1 lg:h-9 lg:w-9"
                  title="移除"
                  aria-label={`移除 ${a.name}`}
                >
                  <X size={11} />
                </button>
              </figure>
            ) : (
              <span
                key={a.id}
                className="inline-flex min-h-8 items-center gap-1.5 rounded-full border border-border bg-surface-3 px-2.5 text-[13px] text-gray-300"
                title={a.path}
              >
                <Paperclip size={10} className="text-accent" />
                <span className="max-w-[160px] truncate">{a.name}</span>
                <span className="text-[11px] text-gray-600">
                  {(a.sizeBytes / 1024).toFixed(0)}KB
                </span>
                <button
                  type="button"
                  onClick={() => removeAttachment(a.id)}
                  className="flex h-11 w-11 items-center justify-center rounded-full text-gray-500 hover:bg-status-danger-soft hover:text-status-danger lg:h-9 lg:w-9"
                  title="移除"
                  aria-label={`移除 ${a.name}`}
                >
                  <X size={10} />
                </button>
              </span>
            )
          ))}
          {uploading && (
            <span className="inline-flex items-center gap-1 text-[13px] text-gray-500">
              <Loader2 size={10} className="animate-spin motion-reduce:animate-none" /> 保存中…
            </span>
          )}
          {attachError && (
            <span className="text-[13px] text-status-danger">{attachError}</span>
          )}
        </div>
      )}
      <div
        data-testid="message-input-control-row"
        className="group rounded-2xl border border-control-border bg-surface-2 shadow-sm transition-colors focus-within:border-accent focus-within:shadow-md"
      >
        <div className="flex items-end gap-2 px-3 py-2.5">
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
          aria-label="附加文件"
          className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg text-gray-500 transition-colors enabled:hover:bg-surface-4 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-30 lg:h-9 lg:w-9"
          title={cwd ? "附加文件（图片 / pptx / docx / pdf / xlsx）" : "打开项目后可附加文件"}
        >
          <Paperclip size={16} />
        </button>
        <textarea
          ref={ref}
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setHistPos(0);
            if (guidanceState.kind !== "idle") setGuidanceState({ kind: "idle" });
            autoResize();
          }}
          onKeyDown={onKey}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
            compositionEndedAtRef.current = Date.now();
          }}
          onPaste={onPaste}
          rows={1}
          placeholder={
            dragOver
              ? "松开以附加文件"
              : guidanceActive
              ? "引导当前执行…"
              : disabled
              ? "选择项目，或输入 /cwd <path>"
              : "描述任务或继续对话…"
          }
          className="min-h-8 max-h-[200px] flex-1 resize-none bg-transparent py-1 text-[15px] leading-6 text-gray-200 outline-none placeholder:text-gray-600 disabled:opacity-40"
        />
        {/* Two buttons during streaming: queue-send (default, primary)
            and cancel-stream (secondary, square icon). Outside streaming
            it's just the regular send. */}
        {streaming && submitReady && (
          <button
            type="button"
            onClick={() => void submit({ forceQueue: true })}
            disabled={!submitReady}
            aria-label="排到当前执行之后"
            className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg bg-status-progress-soft text-status-progress transition-colors enabled:hover:brightness-95 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:h-9 lg:w-9"
            title="排到当前执行之后"
          >
            <Send size={16} />
          </button>
        )}
        <button
          type="button"
          onClick={streaming ? onCancel : () => void submit()}
          disabled={!streaming && !submitReady}
          aria-label={streaming ? "停止后续生成" : guidanceActive ? "引导当前执行" : "发送"}
          className={`flex h-11 w-11 shrink-0 items-center justify-center rounded-lg transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-30 lg:h-9 lg:w-9 ${
            streaming
              ? "bg-status-danger-soft text-status-danger hover:brightness-95"
              : submitReady
                ? "bg-accent text-white shadow-sm hover:bg-accent-hover"
                : "text-gray-600"
          }`}
          title={streaming ? "停止后续生成" : "发送(Enter)"}
        >
          {streaming ? (
            <Square size={16} />
          ) : guidanceState.kind === "pending" ? (
            <Loader2 size={16} className="animate-spin motion-reduce:animate-none" />
          ) : (
            <Send size={16} />
          )}
        </button>
        </div>
        <ComposerControlBar
          shortcutHint={guidanceActive
            ? "Enter 引导当前执行 · ⌘Enter 等这轮结束再发 · Shift+Enter 换行"
            : "Enter 发送 · Shift+Enter 换行"}
        >
          {toolbar}
        </ComposerControlBar>
      </div>
      {(streaming || guidanceState.kind !== "idle") && (
      <div className="mt-1 flex min-h-4 items-center gap-2 text-[11px] text-gray-600 select-none">
        {streaming && (
          <span>停止后续生成不会撤销已经完成的修改、提交或推送</span>
        )}
        {guidanceState.kind === "success" && (
          <span aria-live="polite" className="flex items-center gap-1 text-status-success">
            <Check size={11} /> 已送出
          </span>
        )}
        {guidanceState.kind === "error" && (
          <span role="alert" className="truncate text-status-danger">
            引导发送失败：{guidanceState.message}
          </span>
        )}
      </div>
      )}
    </div>
  );
}
