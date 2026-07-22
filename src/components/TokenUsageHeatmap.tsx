// SPDX-License-Identifier: Apache-2.0
import { useEffect, useMemo, useRef, useState } from "react";

export interface UsageHeatmapDay {
  local_date: string;
  status: "recorded" | "partial" | "missing";
  total_tokens: number | null;
  input_tokens?: number;
  output_tokens?: number;
  reasoning_tokens?: number;
  cached_tokens?: number;
  requests: number | null;
}

export function formatUsageTokens(value: number): string {
  if (value < 1_000) return String(value);
  if (value < 1_000_000) {
    const digits = value >= 10_000 ? 0 : 1;
    return `${(value / 1_000).toFixed(digits)}K`;
  }
  return `${(value / 1_000_000).toFixed(2)}M`;
}

export type UsageHeatmapMetric = "tokens" | "budget" | "requests";

export function usageDayAriaLabel(day: UsageHeatmapDay, today: boolean, overBudget: boolean): string {
  if (day.status === "missing") return `${day.local_date}，数据缺失`;
  const tokens = day.total_tokens ?? 0;
  const provenance = day.status === "partial" ? "历史回填，数据可能不完整" : "已记录";
  return `${day.local_date}，${formatUsageTokens(tokens)} Tokens，${provenance}，${day.requests ?? 0} 次请求${today ? "，今天" : ""}${overBudget ? "，超过预算" : ""}`;
}

function intensityClass(total: number | null, max: number): string {
  if (!total) return "bg-surface-3";
  const normalized = Math.log10(total + 1) / Math.log10(max + 1);
  if (normalized < 0.25) return "bg-accent/20";
  if (normalized < 0.5) return "bg-accent/40";
  if (normalized < 0.75) return "bg-accent/65";
  return "bg-accent";
}

function metricIntensityClass(value: number, max: number, metric: UsageHeatmapMetric): string {
  if (metric !== "budget") return intensityClass(value, max);
  if (value <= 0) return "bg-surface-3";
  if (value < 0.25) return "bg-accent/20";
  if (value < 0.5) return "bg-accent/40";
  if (value < 0.8) return "bg-accent/65";
  if (value <= 1) return "bg-accent";
  return "bg-amber-500";
}

interface Props {
  days: UsageHeatmapDay[];
  ariaLabel: string;
  compact?: boolean;
  selectedDate?: string | null;
  onSelectDate?: (localDate: string | null) => void;
  metric?: UsageHeatmapMetric;
  dailyBudgetLimit?: number;
}

export function TokenUsageHeatmap({
  days,
  ariaLabel,
  compact = false,
  selectedDate,
  onSelectDate,
  metric = "tokens",
  dailyBudgetLimit = 0,
}: Props) {
  const metricValues = useMemo(
    () => days.map((day) => metric === "requests"
      ? day.requests ?? 0
      : metric === "budget" && dailyBudgetLimit > 0
        ? (day.total_tokens ?? 0) / dailyBudgetLimit
        : day.total_tokens ?? 0),
    [dailyBudgetLimit, days, metric],
  );
  const max = Math.max(1, ...metricValues);
  const initialIndex = Math.max(0, selectedDate ? days.findIndex((day) => day.local_date === selectedDate) : 0);
  const [activeIndex, setActiveIndex] = useState(initialIndex);
  const cellRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const interactive = Boolean(onSelectDate);
  const todayDate = days.length > 0 ? days[days.length - 1]?.local_date : undefined;
  const cellSize = compact ? "10px" : "12px";
  const monthRows = useMemo(() => {
    const grouped = new Map<string, UsageHeatmapDay[]>();
    for (const day of days) {
      const month = day.local_date.slice(0, 7);
      grouped.set(month, [...(grouped.get(month) ?? []), day]);
    }
    return Array.from(grouped.entries()).reverse();
  }, [days]);

  useEffect(() => {
    const selectedIndex = selectedDate ? days.findIndex((day) => day.local_date === selectedDate) : -1;
    setActiveIndex((current) => selectedIndex >= 0 ? selectedIndex : Math.min(current, Math.max(0, days.length - 1)));
  }, [days, selectedDate]);

  const moveFocus = (nextIndex: number) => {
    const bounded = Math.max(0, Math.min(days.length - 1, nextIndex));
    setActiveIndex(bounded);
    cellRefs.current[bounded]?.focus();
  };

  return (
    <div className="overflow-x-auto pb-1">
      <div
        role="grid"
        aria-label={ariaLabel}
        aria-rowcount={7}
        aria-colcount={Math.ceil(days.length / 7)}
        className="inline-grid grid-flow-col gap-1"
        style={{
          gridAutoColumns: cellSize,
          gridTemplateRows: `repeat(7, ${cellSize})`,
        }}
      >
        {days.map((day, index) => {
          const selected = selectedDate === day.local_date;
          const missing = day.status === "missing";
          const zero = day.status !== "missing" && (day.total_tokens ?? 0) === 0;
          const today = day.local_date === todayDate;
          const overBudget = dailyBudgetLimit > 0 && (day.total_tokens ?? 0) > dailyBudgetLimit;
          const metricValue = metricValues[index] ?? 0;
          const label = usageDayAriaLabel(day, today, overBudget);
          return (
            <button
              type="button"
              role="gridcell"
              key={day.local_date}
              aria-label={label}
              aria-selected={selected}
              tabIndex={interactive && index === activeIndex ? 0 : -1}
              ref={(node) => { cellRefs.current[index] = node; }}
              title={label}
              onClick={() => onSelectDate?.(day.local_date)}
              onFocus={() => { if (interactive) setActiveIndex(index); }}
              onKeyDown={(event) => {
                if (!interactive) return;
                let nextIndex: number | null = null;
                if (event.key === "ArrowUp") nextIndex = index - 1;
                if (event.key === "ArrowDown") nextIndex = index + 1;
                if (event.key === "ArrowLeft") nextIndex = index - 7;
                if (event.key === "ArrowRight") nextIndex = index + 7;
                if (event.key === "Home") nextIndex = index - (index % 7);
                if (event.key === "End") nextIndex = Math.min(days.length - 1, index + (6 - (index % 7)));
                if (event.key === "Escape") {
                  event.preventDefault();
                  onSelectDate?.(null);
                  cellRefs.current[index]?.focus();
                  return;
                }
                if (nextIndex != null) {
                  event.preventDefault();
                  moveFocus(nextIndex);
                }
              }}
              className={`relative aspect-square min-h-2 rounded-[2px] border transition-transform hover:scale-125 focus:scale-125 ${
                missing
                  ? "border-dashed border-gray-600 bg-transparent"
                  : `${metricIntensityClass(metricValue, max, metric)} border-transparent`
              } ${selected ? "ring-2 ring-accent ring-offset-1 ring-offset-surface-1" : ""} ${today ? "outline outline-1 outline-white/70" : ""} ${overBudget ? "border-amber-400" : ""}`}
            >
              {missing && (
                <span aria-hidden className="absolute inset-0 flex items-center justify-center text-[7px] text-gray-500">
                  ×
                </span>
              )}
              {zero && (
                <span aria-hidden className="absolute inset-0 flex items-center justify-center text-[6px] text-gray-600">
                  ·
                </span>
              )}
              {day.status === "partial" && (
                <span aria-hidden className="absolute right-0 top-0 h-1 w-1 rounded-full bg-amber-400" />
              )}
              {overBudget && (
                <span aria-hidden className="absolute -right-0.5 -top-1 text-[7px] font-bold text-amber-700 dark:text-amber-300">!</span>
              )}
            </button>
          );
        })}
      </div>
      {!compact && (
        <div aria-label="按月用量列表" className="mt-3 space-y-1 sm:hidden">
          {monthRows.map(([month, monthDays]) => {
            const recorded = monthDays.filter((day) => day.total_tokens != null);
            const total = recorded.reduce((sum, day) => sum + (day.total_tokens ?? 0), 0);
            const missing = monthDays.length - recorded.length;
            return (
              <button
                key={month}
                type="button"
                onClick={() => onSelectDate?.(monthDays[monthDays.length - 1]?.local_date ?? null)}
                className="flex w-full items-center justify-between rounded border border-border bg-surface-2 px-2.5 py-2 text-xs"
              >
                <span className="text-gray-300">{month}</span>
                <span className="font-mono text-gray-500">{formatUsageTokens(total)}{missing > 0 ? ` · ${missing} 天缺失` : ""}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function UsageHeatmapLegend() {
  return (
    <div className="flex flex-wrap items-center gap-3 text-[10px] text-gray-500" aria-label="地图图例">
      <span className="inline-flex items-center gap-1"><span className="h-2.5 w-2.5 rounded-[2px] border border-gray-600">×</span>数据缺失</span>
      <span className="inline-flex items-center gap-1"><span className="h-2.5 w-2.5 rounded-[2px] bg-surface-3 text-center leading-[10px]">·</span>0 用量</span>
      <span className="inline-flex items-center gap-1"><span className="h-2.5 w-2.5 rounded-[2px] bg-accent/20" />低</span>
      <span className="inline-flex items-center gap-1"><span className="h-2.5 w-2.5 rounded-[2px] bg-accent" />高</span>
      <span className="inline-flex items-center gap-1"><span className="relative h-2.5 w-2.5 rounded-[2px] bg-accent/40"><span className="absolute right-0 top-0 h-1 w-1 rounded-full bg-amber-400" /></span>历史回填</span>
    </div>
  );
}
