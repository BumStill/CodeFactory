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
      {/*
        No `ml-auto` here. The row's free space is claimed by exactly one
        element, and that element is supplied by the caller: the draft bar is
        `flex-1`, and the session toolbar wraps its usage meter in `ml-auto`.
        A second auto margin does not win the right edge — it *splits* the free
        space with the first one, which parked the usage meter (a bare spinner
        while it loads) in the middle of the bar with nothing either side of it.
      */}
      <span
        data-testid="composer-shortcut-hint"
        className="hidden shrink-0 text-caption text-gray-600 select-none lg:group-focus-within:block"
      >
        {shortcutHint}
      </span>
    </div>
  );
}
