// SPDX-License-Identifier: Apache-2.0

export type SlashCommandName = "clear" | "model" | "cwd" | "cost" | "help";

export interface SlashCommandDefinition {
  name: SlashCommandName;
  usage: string;
  description: string;
  argumentHint?: string;
}

export interface ParsedSlashCommand {
  name: string;
  args: string;
  raw: string;
}

export const SLASH_COMMANDS: SlashCommandDefinition[] = [
  {
    name: "clear",
    usage: "/clear",
    description: "Clear the visible conversation.",
  },
  {
    name: "model",
    usage: "/model <id>",
    description: "Set the active model.",
    argumentHint: "<id>",
  },
  {
    name: "cwd",
    usage: "/cwd <path>",
    description: "Open or switch to a project folder session.",
    argumentHint: "<path>",
  },
  {
    name: "cost",
    usage: "/cost",
    description: "Show local token and cost estimates.",
  },
  {
    name: "help",
    usage: "/help",
    description: "Show available slash commands.",
  },
];

export function parseSlashCommand(input: string): ParsedSlashCommand | null {
  const raw = input.trim();
  if (!raw.startsWith("/")) return null;

  const body = raw.slice(1).trimStart();
  const firstWhitespace = body.search(/\s/);
  const name = (firstWhitespace === -1 ? body : body.slice(0, firstWhitespace)).toLowerCase();
  const args = firstWhitespace === -1 ? "" : body.slice(firstWhitespace).trim();

  return { name, args, raw };
}

export function filterSlashCommandSuggestions(input: string): SlashCommandDefinition[] {
  if (!input.startsWith("/") || input.includes("\n")) return [];

  const query = input.slice(1);
  if (/\s/.test(query)) return [];

  const normalized = query.toLowerCase();
  return SLASH_COMMANDS.filter((command) => command.name.startsWith(normalized));
}

export function isKnownSlashCommand(name: string): name is SlashCommandName {
  return SLASH_COMMANDS.some((command) => command.name === name);
}

export function formatHelpFeedback(): string {
  return [
    "Available slash commands:",
    ...SLASH_COMMANDS.map((command) => `- \`${command.usage}\` - ${command.description}`),
  ].join("\n");
}

export function formatCostFeedback(
  activeModel: string,
  inputTokenTotal: number,
  outputTokenTotal: number,
): string {
  const cost = estimateTokenCost(activeModel, inputTokenTotal, outputTokenTotal);
  return [
    "Current local usage estimate:",
    `- Model: \`${activeModel}\``,
    `- Input tokens: ${inputTokenTotal.toLocaleString()}`,
    `- Output tokens: ${outputTokenTotal.toLocaleString()}`,
    `- Estimated cost: ${cost == null ? "not available yet" : `$${cost}`}`,
  ].join("\n");
}

export function usageForSlashCommand(name: SlashCommandName): string {
  return SLASH_COMMANDS.find((command) => command.name === name)?.usage ?? `/${name}`;
}

function estimateTokenCost(
  model: string,
  inputTokenTotal: number,
  outputTokenTotal: number,
): string | null {
  if (inputTokenTotal === 0 && outputTokenTotal === 0) return null;
  const isOpus = model.includes("opus") || model.includes("gpt-4");
  const inputPrice = isOpus ? 3 : 0.5;
  const outputPrice = isOpus ? 15 : 1.5;
  const cost =
    (inputTokenTotal / 1_000_000) * inputPrice +
    (outputTokenTotal / 1_000_000) * outputPrice;
  return cost.toFixed(4);
}
