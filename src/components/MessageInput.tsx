// SPDX-License-Identifier: Apache-2.0
import { useRef, useState, useEffect, KeyboardEvent } from "react";
import { Send, Square } from "lucide-react";
import {
  filterSlashCommandSuggestions,
  parseSlashCommand,
  type ParsedSlashCommand,
} from "../stores/slashCommands";

interface SkillSlashCommand {
  name: string;
  description: string;
  template: string;
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
}

export function MessageInput({ onSend, onCommand, onCancel, streaming, disabled, pendingInsert, onInsertConsumed, skillSlashCommands = [] }: Props) {
  const [value, setValue] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!pendingInsert) return;
    setValue((prev) => (prev ? `${prev} ${pendingInsert}` : pendingInsert));
    onInsertConsumed?.();
    ref.current?.focus();
  }, [pendingInsert]);

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
  const canSubmit = Boolean(value.trim()) && (!disabled || Boolean(commandCandidate));

  const submit = () => {
    const text = value.trim();
    if (!text || streaming) return;
    const command = parseSlashCommand(text);
    if (!command && disabled) return;
    setValue("");
    ref.current!.style.height = "auto";
    if (command && onCommand) {
      void onCommand(command);
      return;
    }
    onSend(text);
  };

  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  };

  const autoResize = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = Math.min(el.scrollHeight, 200) + "px";
  };

  return (
    <div className="border-t border-border bg-surface-1 px-4 py-3">
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
      <div className="flex items-end gap-2 rounded-xl border border-border bg-surface-2 px-3 py-2 focus-within:border-accent/50 transition-colors">
        <textarea
          ref={ref}
          value={value}
          onChange={(e) => { setValue(e.target.value); autoResize(); }}
          onKeyDown={onKey}
          rows={1}
          placeholder={disabled ? "Message or /cwd <path>" : "Message"}
          className="flex-1 resize-none bg-transparent text-sm text-gray-200 placeholder-gray-600 outline-none min-h-[24px] max-h-[200px] leading-6 disabled:opacity-40"
        />
        <button
          onClick={streaming ? onCancel : submit}
          disabled={!streaming && !canSubmit}
          className="shrink-0 rounded-lg p-1.5 transition-colors disabled:opacity-30
            enabled:hover:bg-surface-4 text-accent disabled:text-gray-600"
          title={streaming ? "Cancel" : "Send (Enter)"}
        >
          {streaming ? <Square size={16} /> : <Send size={16} />}
        </button>
      </div>
      <div className="mt-1 text-xs text-gray-700 text-right select-none">
        Enter to send · Shift+Enter for newline
      </div>
    </div>
  );
}
