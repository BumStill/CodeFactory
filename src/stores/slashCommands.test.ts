// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import {
  filterSlashCommandSuggestions,
  formatCostFeedback,
  formatHelpFeedback,
  parseSlashCommand,
} from "./slashCommands";

describe("slash commands", () => {
  it("parses the model command and preserves the model id", () => {
    const parsed = parseSlashCommand("/model anthropic/claude-sonnet-4");
    expect(parsed).toBeTruthy();
    expect(parsed?.name).toBe("model");
    expect(parsed?.args).toBe("anthropic/claude-sonnet-4");
  });

  it("does not treat plain messages containing slash text as commands", () => {
    expect(parseSlashCommand("please run /clear")).toBeNull();
  });

  it("opens all command suggestions on a bare slash", () => {
    const suggestions = filterSlashCommandSuggestions("/");
    expect(suggestions).toHaveLength(5);
    expect(suggestions[0].name).toBe("clear");
  });

  it("filters suggestions for a partial command", () => {
    const suggestions = filterSlashCommandSuggestions("/c");
    expect(suggestions).toHaveLength(3);
    expect(suggestions.map((command) => command.name).join(",")).toBe("clear,cwd,cost");
  });

  it("lists command usage in help feedback", () => {
    const help = formatHelpFeedback();
    expect(help).toContain("/clear");
    expect(help).toContain("/model <id>");
    expect(help).toContain("/cwd <path>");
  });

  it("includes model, token counts, and estimated price in cost feedback", () => {
    const cost = formatCostFeedback("anthropic/claude-opus-4-7", 1_000, 2_000);
    expect(cost).toContain("anthropic/claude-opus-4-7");
    expect(cost).toContain("1,000");
    expect(cost).toContain("2,000");
    expect(cost).toContain("$0.0330");
  });
});
