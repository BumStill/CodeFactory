// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useRef, useState } from "react";
import { usageDayAriaLabel, type UsageHeatmapDay } from "./TokenUsageHeatmap";

interface Props {
  days: UsageHeatmapDay[];
  ariaLabel: string;
  dailyBudgetLimit?: number;
}

function tone(value: number, max: number): string {
  if (value <= 0) return "bg-accent/20";
  const normalized = Math.log10(value + 1) / Math.log10(max + 1);
  if (normalized < 0.25) return "bg-accent/30";
  if (normalized < 0.5) return "bg-accent/50";
  if (normalized < 0.75) return "bg-accent/70";
  return "bg-accent";
}

/**
 * Welcome-only chronological summary. This intentionally does not reuse the
 * 7-row Settings calendar: a compact trend and a drillable calendar have
 * different visual density and keyboard topology.
 */
export function TokenUsageTrend({ days, ariaLabel, dailyBudgetLimit = 0 }: Props) {
  const values = useMemo(() => days.map((day) => day.total_tokens ?? 0), [days]);
  const max = Math.max(1, ...values);
  const [activeIndex, setActiveIndex] = useState(Math.max(0, days.length - 1));
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const todayDate = days[days.length - 1]?.local_date;

  useEffect(() => {
    setActiveIndex((current) => Math.min(current, Math.max(0, days.length - 1)));
  }, [days.length]);

  const moveFocus = (nextIndex: number) => {
    const bounded = Math.max(0, Math.min(days.length - 1, nextIndex));
    setActiveIndex(bounded);
    refs.current[bounded]?.focus();
  };

  return (
    <div
      role="grid"
      aria-label={ariaLabel}
      aria-rowcount={1}
      aria-colcount={days.length}
      className="grid h-10 w-full items-end gap-[3px]"
      style={{ gridTemplateColumns: `repeat(${Math.max(days.length, 1)}, minmax(4px, 1fr))` }}
    >
      {days.map((day, index) => {
        const missing = day.status === "missing";
        const value = day.total_tokens ?? 0;
        const zero = !missing && value === 0;
        const today = day.local_date === todayDate;
        const overBudget = dailyBudgetLimit > 0 && value > dailyBudgetLimit;
        const normalized = value > 0 ? Math.sqrt(value / max) : 0;
        const height = missing || zero ? 4 : Math.round(5 + normalized * 31);
        const label = usageDayAriaLabel(day, today, overBudget);
        return (
          <button
            key={day.local_date}
            ref={(node) => { refs.current[index] = node; }}
            type="button"
            role="gridcell"
            aria-label={label}
            aria-current={today ? "date" : undefined}
            tabIndex={index === activeIndex ? 0 : -1}
            title={label}
            onFocus={() => setActiveIndex(index)}
            onKeyDown={(event) => {
              let nextIndex: number | null = null;
              if (event.key === "ArrowLeft") nextIndex = index - 1;
              if (event.key === "ArrowRight") nextIndex = index + 1;
              if (event.key === "Home") nextIndex = 0;
              if (event.key === "End") nextIndex = days.length - 1;
              if (nextIndex != null) {
                event.preventDefault();
                moveFocus(nextIndex);
              }
            }}
            className={`w-full rounded-[2px] border border-transparent transition-colors focus:outline-none focus:ring-2 focus:ring-accent focus:ring-offset-1 focus:ring-offset-surface-1 ${tone(value, max)}`}
            style={{ height: `${height}px` }}
          />
        );
      })}
    </div>
  );
}
