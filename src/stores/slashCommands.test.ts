// SPDX-License-Identifier: Apache-2.0
import {
  filterSlashCommandSuggestions,
  formatCostFeedback,
  formatHelpFeedback,
  parseSlashCommand,
} from "./slashCommands.js";

function assertEqual<T>(actual: T, expected: T, label: string) {
  if (actual !== expected) {
    throw new Error(`${label}: expected ${String(expected)}, got ${String(actual)}`);
  }
}

function assertTruthy(value: unknown, label: string): asserts value {
  if (!value) {
    throw new Error(`${label}: expected truthy value`);
  }
}

function assertIncludes(actual: string, expected: string, label: string) {
  if (!actual.includes(expected)) {
    throw new Error(`${label}: expected ${JSON.stringify(actual)} to include ${JSON.stringify(expected)}`);
  }
}

{
  const parsed = parseSlashCommand("/model anthropic/claude-sonnet-4");
  assertTruthy(parsed, "model command parses");
  assertEqual(parsed.name, "model", "model command name");
  assertEqual(parsed.args, "anthropic/claude-sonnet-4", "model command args preserve model id");
}

{
  const parsed = parseSlashCommand("please run /clear");
  assertEqual(parsed, null, "plain messages with slash text are not commands");
}

{
  const suggestions = filterSlashCommandSuggestions("/");
  assertEqual(suggestions.length, 5, "slash opens all command suggestions");
  assertEqual(suggestions[0].name, "clear", "clear is the first lightweight command");
}

{
  const suggestions = filterSlashCommandSuggestions("/c");
  assertEqual(suggestions.length, 3, "partial command filters suggestions");
  assertEqual(suggestions.map((command) => command.name).join(","), "clear,cwd,cost", "filtered command order");
}

{
  const help = formatHelpFeedback();
  assertIncludes(help, "/clear", "help lists clear");
  assertIncludes(help, "/model <id>", "help lists model usage");
  assertIncludes(help, "/cwd <path>", "help lists cwd usage");
}

{
  const cost = formatCostFeedback("anthropic/claude-opus-4-7", 1_000, 2_000);
  assertIncludes(cost, "anthropic/claude-opus-4-7", "cost includes active model");
  assertIncludes(cost, "1,000", "cost includes input tokens");
  assertIncludes(cost, "2,000", "cost includes output tokens");
  assertIncludes(cost, "$0.0330", "cost includes estimated price");
}
