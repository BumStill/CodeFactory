// SPDX-License-Identifier: Apache-2.0
// CF-WINDOW-R6（M86 返修）：用户在会话里手动选了一个当前不在允许使用时段内的
// 模型时，提示必须是**应用内对话框**。
//
// 为什么不能用 window.alert / window.confirm：Tauri WebView 里浏览器原生弹窗
// 不保证弹得出来，弹不出时用户点了模型却毫无反应（M86 观察到的现象），而且它
// 和应用其它地方的确认交互风格不一致。这里沿用仓库既有的对话框约定
// （role="dialog" + aria-modal + Esc 关闭 + 焦点回到原处）。
import { useEffect, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

export interface UsageWindowDialogProps {
  endpoint: string;
  model: string;
  /** 时段下一次变化的时间；null 表示当前规则看不到切换点。 */
  nextTransitionMs: number | null;
  /** 本会话忽略时段限制（一次性）。没有会话可归属时为 null，只解释不提供选项。 */
  onIgnoreForSession: (() => void) | null;
  onCancel: () => void;
  /** 同仓库其它确认对话框的约定：验收页要并排看多个状态时用嵌入式定位。 */
  embedded?: boolean;
}

function clockText(ms: number | null): string | null {
  if (ms == null) return null;
  const at = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

export function UsageWindowDialog({
  endpoint,
  model,
  nextTransitionMs,
  onIgnoreForSession,
  onCancel,
  embedded = false,
}: UsageWindowDialogProps) {
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const returnTarget = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelRef.current?.focus();
    return () => {
      if (returnTarget?.isConnected) returnTarget.focus();
    };
  }, []);

  const clock = clockText(nextTransitionMs);
  const onKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      onCancel();
      return;
    }
    // 把焦点留在对话框里：它是模态的，Tab 不该跑到后面的会话上。
    if (event.key === "Tab") {
      const focusables = event.currentTarget.querySelectorAll<HTMLElement>("button:not([disabled])");
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  };

  return (
    <div
      className={`${embedded ? "absolute" : "fixed"} inset-0 z-[120] flex items-center justify-center bg-black/60 p-3`}
      onClick={onCancel}
    >
      <div
        role="dialog"
        aria-modal={embedded ? "false" : "true"}
        aria-label={`${model} 不在允许使用时段`}
        data-testid="usage-window-dialog"
        className="flex w-[min(520px,92vw)] flex-col rounded-xl border border-border bg-surface-2 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <header className="border-b border-border px-4 py-3">
          <h2 className="text-body font-semibold text-gray-100">{model} 不在允许使用时段</h2>
          <p className="mt-0.5 text-caption text-gray-500">
            {endpoint} / {model}
          </p>
        </header>
        <div className="space-y-2 px-4 py-3 text-label leading-relaxed text-gray-300">
          <p>
            你为这个模型配置了使用时段，现在不在允许时间内
            {clock ? `（下一次变化：${clock}）` : "（当前规则看不到下一次切换时间）"}。
          </p>
          <p className="text-gray-500">
            不会静默改用别的模型，也不会静默违反你的配置。可以改选其它模型，或在设置里放开时段。
          </p>
          {onIgnoreForSession ? (
            <p className="text-gray-500">
              「本会话忽略时段限制」只对当前这一个会话生效，别的会话和之后新建的会话仍按配置执行；它会被保存下来，
              重启后依然有效，随时可以从模型菜单里撤销。
            </p>
          ) : (
            <p className="text-gray-500">当前不在任何会话里，所以没有可以一次性忽略的会话范围。</p>
          )}
        </div>
        <footer className="flex flex-wrap justify-end gap-2 border-t border-border px-4 py-3">
          <button
            ref={cancelRef}
            type="button"
            className="min-h-[44px] rounded border border-control-border px-3 py-1 text-label text-gray-300 transition-colors hover:bg-surface-3 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
            onClick={onCancel}
          >
            取消，改选其它模型
          </button>
          {onIgnoreForSession && (
            <button
              type="button"
              className="min-h-[44px] rounded border border-accent/40 bg-accent/10 px-3 py-1 text-label font-medium text-gray-100 transition-colors hover:bg-accent/20 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent lg:min-h-[36px]"
              onClick={onIgnoreForSession}
            >
              本会话忽略时段限制
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}
