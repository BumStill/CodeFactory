// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MessageInput } from "./MessageInput";

vi.mock("../lib/tauri", () => ({ invoke: vi.fn() }));

describe("MessageInput cancellation contract", () => {
  it("explains that stopping future generation does not roll back completed work", () => {
    render(
      <MessageInput
        onSend={vi.fn()}
        onCancel={vi.fn()}
        streaming={true}
        disabled={false}
        cwd="/proj"
      />,
    );

    expect(screen.getByTitle("停止后续生成")).toBeInTheDocument();
    expect(
      screen.getByText("停止后续生成不会撤销已经完成的修改、提交或推送"),
    ).toBeInTheDocument();
  });
});
