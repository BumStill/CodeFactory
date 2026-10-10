// SPDX-License-Identifier: Apache-2.0
//
// 会话的交付记录（分支 + PR 号）。
//
// 从 `components/WorkspaceDeliveryStatus.tsx` 挪到 `lib/`：派单入口的 `status`
// （CF-HDE-R9）要返回 pr_number，而 store/lib 不该反向依赖组件。组件改为原样
// 转出这里的实现，界面与入口读的是同一份解析规则。

import type { UIMessage } from "../stores/chatEvents";

export interface DeliveryReference {
  branch: string;
  prNumber: number;
}

/** Last successful delivery call is a compatibility fallback for conversations
 *  created before session_delivery_refs existed. New calls persist this relation
 *  in SQLite, so it survives returning the checkout to main. */
export function deliveryReferenceFromMessages(messages: UIMessage[]): DeliveryReference | null {
  for (let messageIndex = messages.length - 1; messageIndex >= 0; messageIndex -= 1) {
    const calls = messages[messageIndex].toolCalls ?? [];
    for (let callIndex = calls.length - 1; callIndex >= 0; callIndex -= 1) {
      const call = calls[callIndex];
      if (call.name !== "deliver_changes" || !call.result) continue;
      const branch = call.result.match(/^分支:\s*(.+)$/m)?.[1]?.trim();
      const prNumber = Number(call.result.match(/(?:PR\s*#|\/pull\/)(\d+)/)?.[1] ?? 0);
      if (branch && prNumber > 0) return { branch, prNumber };
    }
  }
  return null;
}
