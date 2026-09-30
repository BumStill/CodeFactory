// SPDX-License-Identifier: Apache-2.0
import { formatRelativeTime } from "./time";

/** 会话 id 的前 8 位。
 *
 * 侧边栏里一排同名「新会话」在无障碍树里是完全一样的 AXButton:读屏软件、
 * 后台无障碍工具都无法区分它们,只能靠"第几个按钮"这种脆弱定位。把短 id
 * 放进无障碍名称,同名会话就有了稳定的区分依据。视觉上不显示 id —— 那是给
 * 机器读的,不是给人读的。 */
export function shortSessionId(id: string): string {
  return id.slice(0, 8);
}

/** 无障碍名称里的会话标识:`标题(短 id,相对时间)`。
 *
 * 例:`新会话(9537257c,5 分钟前)`。菜单栏的「切换会话」用同一份文案,所以
 * 用户在菜单里认人、在侧边栏认人,靠的是同一串字符。 */
export function sessionAccessibleName(
  title: string,
  id: string,
  updatedAt: number,
  now: number = Date.now(),
): string {
  return `${title}(${shortSessionId(id)},${formatRelativeTime(updatedAt, now)})`;
}
