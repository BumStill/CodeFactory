// SPDX-License-Identifier: Apache-2.0
//! 菜单栏的静态描述。
//!
//! 这里只放"数据":菜单标题、每项的稳定 id、用户可见文案和快捷键。真正的
//! 原生菜单构建在 `menu.rs`,前端分发在 `src/lib/desktopMenu.ts`。
//!
//! 为什么要拆出来:无障碍后台操作要求菜单项"点了就生效",而菜单项的可用
//! 状态、勾选状态必须和界面完全一致。把结构与快捷键放在一张纯数据表里,
//! 就能用单元测试直接断言结构与快捷键(不需要启动 Tauri runtime),前端也
//! 能用同一份 id 契约交叉校验(见 `src/lib/desktopMenu.test.ts`)。
//!
//! 约束:所有菜单项都只是信封。点击后原生侧只发一个事件给前端,由前端调用
//! 和界面按钮完全相同的那一条代码路径,绝不在这里另写一套业务逻辑。

/// 顶层菜单标题。用户可见文案,保持中文。
pub const SESSION_MENU_TITLE: &str = "会话";

/// "切换会话"子菜单标题。
pub const SWITCH_SUBMENU_TITLE: &str = "切换会话";

/// "权限模式"子菜单标题。
pub const PERMISSION_SUBMENU_TITLE: &str = "权限模式";

/// 新建会话(当前项目)。
pub const ACTION_NEW_SESSION: &str = "session.new";

/// 切换到某个会话。真实 id 通过事件 payload 的 `sessionId` 传下去。
pub const ACTION_SWITCH_SESSION: &str = "session.switch";

/// 切换当前会话的权限模式。模式通过事件 payload 的 `mode` 传下去。
pub const ACTION_SET_PERMISSION_MODE: &str = "session.permission";

/// 聚焦输入框。
pub const ACTION_FOCUS_INPUT: &str = "session.focus-input";

/// 发送输入框当前内容。
pub const ACTION_SEND_INPUT: &str = "session.send";

/// 从剪贴板发送到当前会话(原生侧读剪贴板,文本随 payload 下发)。
pub const ACTION_SEND_CLIPBOARD: &str = "session.send-clipboard";

/// 停止当前执行。
pub const ACTION_STOP_RUN: &str = "session.stop";

/// 清理编译缓存。走原生侧直接调用与界面按钮同一条 Rust 命令,
/// 因此锁屏、窗口不在前台时同样可用(CF-BLD-R4 的后台入口)。
pub const ACTION_CLEAN_BUILD_CACHE: &str = "session.clean-build-cache";

/// 「切换会话」子菜单里最多放多少个会话。够用即可:再长的列表放进菜单也
/// 找不到,而且原生菜单项是启动时一次性建好的,固定上限避免动态增删。
pub const RECENT_SESSION_LIMIT: usize = 20;

/// 三个权限模式:值必须与 `PermissionMode`(`safe` / `standard` / `trusted`)
/// 一字不差,后端 `validate_permission_mode` 会拒绝其它取值。
pub const PERMISSION_MODES: &[(&str, &str)] = &[
    ("safe", "安全"),
    ("standard", "标准"),
    ("trusted", "信任"),
];

/// 一个普通菜单项(非勾选、非子菜单)的静态描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItemSpec {
    /// 稳定 id,同时是派发给前端的事件标签。
    pub id: &'static str,
    /// 用户可见文案。
    pub label: &'static str,
    /// 快捷键。`None` 表示没有快捷键(例如剪贴板发送、停止执行)。
    pub accelerator: Option<&'static str>,
}

/// 会话菜单里顺序固定的普通项。
///
/// 「新建会话」与「切换会话」之间不放分隔符以外的动态内容:动态的切换列表
/// 是独立的子菜单,启动时一次性建成。
pub const ACTION_ITEMS: &[MenuItemSpec] = &[
    MenuItemSpec {
        id: ACTION_NEW_SESSION,
        label: "新建会话(当前项目)",
        accelerator: Some("CmdOrCtrl+N"),
    },
    MenuItemSpec {
        id: ACTION_FOCUS_INPUT,
        label: "聚焦输入框",
        accelerator: Some("CmdOrCtrl+L"),
    },
    MenuItemSpec {
        id: ACTION_SEND_INPUT,
        label: "发送输入框内容",
        accelerator: Some("CmdOrCtrl+Return"),
    },
    MenuItemSpec {
        id: ACTION_SEND_CLIPBOARD,
        label: "从剪贴板发送到当前会话",
        accelerator: None,
    },
    MenuItemSpec {
        id: ACTION_STOP_RUN,
        label: "停止当前执行",
        accelerator: None,
    },
    MenuItemSpec {
        id: ACTION_CLEAN_BUILD_CACHE,
        label: "清理编译缓存",
        accelerator: None,
    },
];

/// 原生菜单项 id 的前缀:切换列表的第 n 项。
pub fn switch_item_id(index: usize) -> String {
    format!("{ACTION_SWITCH_SESSION}.{index}")
}

/// 原生菜单项 id 的前缀:权限模式的某一项。
pub fn permission_item_id(mode: &str) -> String {
    format!("{ACTION_SET_PERMISSION_MODE}.{mode}")
}

/// 由稳定 id 还原出"这是哪个动作",丢掉动态后缀。
///
/// `session.switch.3` → `session.switch`;`session.permission.trusted` →
/// `session.permission`;未知 id 原样返回(交给前端判空忽略)。
pub fn action_of(menu_id: &str) -> &str {
    if let Some(rest) = menu_id.strip_prefix(&format!("{ACTION_SWITCH_SESSION}.")) {
        // `session.switch` 之后的整段都是下标,动作就是它本身。
        let _ = rest;
        return ACTION_SWITCH_SESSION;
    }
    if let Some(rest) = menu_id.strip_prefix(&format!("{ACTION_SET_PERMISSION_MODE}.")) {
        let _ = rest;
        return ACTION_SET_PERMISSION_MODE;
    }
    menu_id
}

/// 由原生菜单 id 还原出动态尾巴(下标或权限模式)。静态项返回 `None`。
pub fn dynamic_suffix(menu_id: &str) -> Option<&str> {
    menu_id
        .strip_prefix(&format!("{ACTION_SWITCH_SESSION}."))
        .or_else(|| menu_id.strip_prefix(&format!("{ACTION_SET_PERMISSION_MODE}.")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_menu_exposes_the_actions_the_menu_bar_promises() {
        let ids: Vec<&str> = ACTION_ITEMS.iter().map(|item| item.id).collect();
        for required in [
            ACTION_NEW_SESSION,
            ACTION_FOCUS_INPUT,
            ACTION_SEND_INPUT,
            ACTION_SEND_CLIPBOARD,
            ACTION_STOP_RUN,
        ] {
            assert!(
                ids.contains(&required),
                "菜单缺少必需项 {required},现有:{ids:?}"
            );
        }
        assert_eq!(
            ids.len(),
            ACTION_ITEMS
                .iter()
                .map(|item| item.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            "菜单项 id 必须唯一"
        );
    }

    #[test]
    fn accelerators_match_the_promised_shortcuts() {
        let by_id = |id: &str| {
            ACTION_ITEMS
                .iter()
                .find(|item| item.id == id)
                .copied()
                .unwrap_or_else(|| panic!("missing menu item {id}"))
        };
        assert_eq!(by_id(ACTION_NEW_SESSION).accelerator, Some("CmdOrCtrl+N"));
        assert_eq!(by_id(ACTION_FOCUS_INPUT).accelerator, Some("CmdOrCtrl+L"));
        assert_eq!(by_id(ACTION_SEND_INPUT).accelerator, Some("CmdOrCtrl+Return"));
        // 后台无障碍工具走的是菜单项本身,不该依赖键盘焦点以外的加速键。
        assert_eq!(by_id(ACTION_SEND_CLIPBOARD).accelerator, None);
        assert_eq!(by_id(ACTION_STOP_RUN).accelerator, None);
    }

    #[test]
    fn labels_are_human_readable_and_free_of_internal_jargon() {
        for item in ACTION_ITEMS {
            assert!(
                !item.label.is_empty() && !item.label.contains('_'),
                "菜单文案应当是可读的中文,实际:{:?}",
                item.label
            );
        }
        assert_eq!(SESSION_MENU_TITLE, "会话");
        assert_eq!(SWITCH_SUBMENU_TITLE, "切换会话");
        assert_eq!(PERMISSION_SUBMENU_TITLE, "权限模式");
        assert_eq!(RECENT_SESSION_LIMIT, 20);
    }

    #[test]
    fn permission_modes_are_the_three_backend_accepted_values() {
        let modes: Vec<&str> = PERMISSION_MODES.iter().map(|(mode, _)| *mode).collect();
        assert_eq!(modes, vec!["safe", "standard", "trusted"]);
        for (_, label) in PERMISSION_MODES {
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn dynamic_menu_ids_round_trip_back_to_their_action() {
        assert_eq!(action_of(&switch_item_id(7)), ACTION_SWITCH_SESSION);
        assert_eq!(action_of(&permission_item_id("trusted")), ACTION_SET_PERMISSION_MODE);
        assert_eq!(action_of(ACTION_NEW_SESSION), ACTION_NEW_SESSION);
        assert_eq!(dynamic_suffix(&switch_item_id(7)), Some("7"));
        assert_eq!(dynamic_suffix(&permission_item_id("safe")), Some("safe"));
        assert_eq!(dynamic_suffix(ACTION_STOP_RUN), None);
        // 前缀本身不能被当成动态项,否则 `session.switch` 会被误解析出空下标。
        assert_eq!(dynamic_suffix(ACTION_SWITCH_SESSION), None);
    }
}
