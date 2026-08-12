// SPDX-License-Identifier: Apache-2.0
import type { KeyboardEvent, ReactNode } from "react";

interface ComposerControlBarProps {
  children: ReactNode;
  shortcutHint: string;
}

/**
 * The single compact control surface owned by the composer. Controls remain
 * reachable at every viewport; only the redundant keyboard reminder is
 * progressively disclosed on a focused desktop composer.
 */
export function ComposerControlBar({ children, shortcutHint }: ComposerControlBarProps) {
  const moveToolbarFocus = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>(
        'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])',
      ),
    );
    if (items.length === 0) return;
    const currentIndex = items.indexOf(document.activeElement as HTMLElement);
    if (currentIndex < 0) return;
    event.preventDefault();
    const nextIndex = event.key === "Home"
      ? 0
      : event.key === "End"
        ? items.length - 1
        : event.key === "ArrowRight"
          ? (currentIndex + 1) % items.length
          : (currentIndex - 1 + items.length) % items.length;
    items[nextIndex]?.focus();
  };

  return (
    <div
      data-testid="composer-utility-toolbar"
      role="toolbar"
      aria-label="输入工具"
      onKeyDown={moveToolbarFocus}
      className="flex min-h-[44px] min-w-0 max-w-full flex-wrap items-center gap-1 overflow-x-clip border-t border-border/60 px-2 py-1 lg:min-h-[36px]"
    >
      {children}
      <span
        data-testid="composer-shortcut-hint"
        className="ml-auto hidden shrink-0 text-[11px] text-gray-600 select-none lg:group-focus-within:block"
      >
        {shortcutHint}
      </span>
    </div>
  );
}
