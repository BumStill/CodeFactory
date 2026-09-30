// SPDX-License-Identifier: Apache-2.0
//! 原生菜单栏:「会话」菜单 + 剪贴板发送通道。
//!
//! 背景:编排方需要在不把窗口切到前台、不接管屏幕的前提下,只靠 macOS 无
//! 障碍工具(后台点击、后台输入、菜单栏)完成日常操作。WKWebView 在非活动窗口
//! 里不处理键盘事件,所以"后台输入文字"不能依赖键盘焦点——菜单栏是唯一在后台
//! 依然可靠的原生入口,配合 `acceptFirstMouse` 让第一下点击直接落到页面上。
//!
//! 设计原则:原生侧只做三件事。
//! 1. 把菜单点击翻译成一个事件(`menu:session`)发给前端;
//! 2. 读取剪贴板这类前端拿不到的原生能力;
//! 3. 按前端同步过来的状态更新菜单项的可用/勾选状态。
//!
//! 业务语义一律留在前端,走和界面按钮完全相同的那条代码路径。

use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::menu_spec as spec;
use crate::util::no_window::NoWindow;

/// 菜单点击事件名。前端 `src/lib/desktopMenu.ts` 监听它。
pub const SESSION_MENU_EVENT: &str = "menu:session";

/// 切换列表里每一行的上限,和 `menu_spec::RECENT_SESSION_LIMIT` 是同一个数。
/// 原生菜单项在启动时一次性建好,之后只改文案,不增删,避免运行时改菜单树。
const SWITCH_SLOTS: usize = spec::RECENT_SESSION_LIMIT;

/// 空槽位的占位文案:用户不该看到"第 13 个会话"这种内部概念。
const EMPTY_SLOT_LABEL: &str = "—";

/// 发给前端的事件负载。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMenuEvent {
    /// `menu_spec` 里的稳定动作 id。
    pub action: String,
    /// `session.switch` 专用:目标会话 id。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// `session.permission` 专用:`safe` / `standard` / `trusted`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// `session.send-clipboard` 专用:原生侧读到的剪贴板文本。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl SessionMenuEvent {
    fn action(action: &str) -> Self {
        Self {
            action: action.to_string(),
            session_id: None,
            mode: None,
            text: None,
        }
    }
}

/// 前端同步过来的一项会话(用于「切换会话」)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMenuEntry {
    /// 会话 id,点击后原样回传。
    pub id: String,
    /// 已排好序的用户可见文案:标题 + (短 id, 相对时间)。
    pub label: String,
    /// 是否是当前打开的会话(打勾)。
    #[serde(default)]
    pub checked: bool,
}

/// 前端同步菜单状态。可用状态和勾选状态必须与界面一致。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMenuState {
    /// 最近会话,按界面顺序(最新的在前),最多 `SWITCH_SLOTS` 项。
    #[serde(default)]
    pub entries: Vec<SessionMenuEntry>,
    /// 当前打开的会话 id(草稿没有 id,传 `None`)。用于给切换列表打勾。
    #[serde(default)]
    pub current_session_id: Option<String>,
    /// 当前会话/草稿的权限模式。
    #[serde(default)]
    pub permission_mode: String,
    /// 有没有当前项目。没有项目时「新建会话(当前项目)」必须灰掉。
    #[serde(default)]
    pub has_project: bool,
    /// 有没有正在进行的执行。没有时「停止当前执行」必须灰掉。
    #[serde(default)]
    pub running: bool,
}

/// 原生菜单句柄。这些句柄在启动时一次性创建,之后只被改写文案与状态。
pub struct SessionMenuHandles {
    pub new_session: MenuItem<Wry>,
    pub focus_input: MenuItem<Wry>,
    pub send_input: MenuItem<Wry>,
    pub send_clipboard: MenuItem<Wry>,
    pub stop_run: MenuItem<Wry>,
    pub switch_items: Vec<CheckMenuItem<Wry>>,
    pub switch_sessions: Vec<Option<String>>,
    pub permission_items: Vec<(String, CheckMenuItem<Wry>)>,
}

/// 放进 Tauri state 的菜单句柄容器。
#[derive(Default)]
pub struct SessionMenuStateHandle(pub std::sync::Mutex<Option<SessionMenuHandles>>);

/// 读取剪贴板文本。
///
/// 不引入新的剪贴板 crate:后台编排只需要"读一段文本",用系统自带的命令行
/// 工具最可靠,也避免为一次读取付出编译时间和额外依赖。
/// 读不到(空剪贴板、非文本内容、命令缺失)一律返回 `None`,由前端按"空内容
/// 不发送"处理。
pub fn read_clipboard_text() -> Option<String> {
    #[cfg(target_os = "macos")]
    let output = Command::new("pbpaste").no_window().output().ok()?;
    #[cfg(target_os = "windows")]
    let output = Command::new("powershell")
        .no_window()
        .args(["-NoProfile", "-Command", "Get-Clipboard -Raw"])
        .output()
        .ok()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let output = Command::new("xclip")
        .no_window()
        .args(["-selection", "clipboard", "-o"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

/// 把原生菜单 id 翻译成发给前端的事件。
///
/// 纯函数,便于测试:菜单栏的每个条目都必须落到界面已有的那条代码路径。
/// `session.switch` 的槽位下标 → 真实会话 id 的替换在 `handle_menu_event` 里
/// 完成(需要菜单状态),这里先带上一个空 id 占位。
pub fn event_for_menu_id(menu_id: &str, clipboard: Option<String>) -> Option<SessionMenuEvent> {
    let action = spec::action_of(menu_id);
    match action {
        spec::ACTION_SWITCH_SESSION => {
            // 槽位下标必须能解析,否则这不是一个合法的切换项。
            spec::dynamic_suffix(menu_id)?.parse::<usize>().ok()?;
            Some(SessionMenuEvent {
                action: action.to_string(),
                session_id: Some(String::new()),
                mode: None,
                text: None,
            })
        }
        spec::ACTION_SET_PERMISSION_MODE => {
            let mode = spec::dynamic_suffix(menu_id)?;
            if !spec::PERMISSION_MODES
                .iter()
                .any(|(known, _)| *known == mode)
            {
                return None;
            }
            Some(SessionMenuEvent {
                action: action.to_string(),
                session_id: None,
                mode: Some(mode.to_string()),
                text: None,
            })
        }
        spec::ACTION_SEND_CLIPBOARD => Some(SessionMenuEvent {
            action: action.to_string(),
            session_id: None,
            mode: None,
            // 剪贴板为空时仍然照常派发,由前端的"空文本不发送"校验兜底 ——
            // 原生侧不重复实现业务校验。
            text: clipboard,
        }),
        spec::ACTION_NEW_SESSION
        | spec::ACTION_FOCUS_INPUT
        | spec::ACTION_SEND_INPUT
        | spec::ACTION_STOP_RUN => Some(SessionMenuEvent::action(action)),
        _ => None,
    }
}

/// 启动时构建并安装「会话」菜单。
///
/// 在系统默认菜单(File / Edit / View / Window)之后追加我们的子菜单:默认
/// 菜单已经带着 macOS 需要的应用菜单与标准编辑项,重写整套反而容易漏项。
pub fn install(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let session_menu = Submenu::new(app, spec::SESSION_MENU_TITLE, true)?;

    let mut by_id: std::collections::HashMap<&'static str, MenuItem<Wry>> = Default::default();
    for item in spec::ACTION_ITEMS {
        let mut builder = tauri::menu::MenuItemBuilder::new(item.label).id(item.id);
        if let Some(accelerator) = item.accelerator {
            builder = builder.accelerator(accelerator);
        }
        by_id.insert(item.id, builder.build(app)?);
    }
    let take = |by_id: &mut std::collections::HashMap<&'static str, MenuItem<Wry>>, id: &str| {
        by_id
            .remove(id)
            .unwrap_or_else(|| panic!("menu_spec 缺少菜单项 {id}"))
    };

    let new_session = take(&mut by_id, spec::ACTION_NEW_SESSION);
    session_menu.append(&new_session)?;
    session_menu.append(&PredefinedMenuItem::separator(app)?)?;

    // 「切换会话 ▸」:固定槽位,启动时建好,之后只改文案/勾选状态。
    // 用勾选项承载"当前会话",这样"选了哪个"在菜单里一眼可见。
    let switch_menu = Submenu::new(app, spec::SWITCH_SUBMENU_TITLE, true)?;
    let mut switch_items = Vec::with_capacity(SWITCH_SLOTS);
    for index in 0..SWITCH_SLOTS {
        let item = tauri::menu::CheckMenuItemBuilder::new(EMPTY_SLOT_LABEL)
            .id(spec::switch_item_id(index))
            .enabled(false)
            .build(app)?;
        switch_menu.append(&item)?;
        switch_items.push(item);
    }
    session_menu.append(&switch_menu)?;

    // 「权限模式 ▸」:单选,勾选当前会话的模式。
    let permission_menu = Submenu::new(app, spec::PERMISSION_SUBMENU_TITLE, true)?;
    let mut permission_items = Vec::with_capacity(spec::PERMISSION_MODES.len());
    for (mode, label) in spec::PERMISSION_MODES {
        let item = tauri::menu::CheckMenuItemBuilder::new(*label)
            .id(spec::permission_item_id(mode))
            .checked(*mode == "standard")
            .build(app)?;
        permission_menu.append(&item)?;
        permission_items.push(((*mode).to_string(), item));
    }
    session_menu.append(&permission_menu)?;
    session_menu.append(&PredefinedMenuItem::separator(app)?)?;

    let focus_input = take(&mut by_id, spec::ACTION_FOCUS_INPUT);
    let send_input = take(&mut by_id, spec::ACTION_SEND_INPUT);
    let send_clipboard = take(&mut by_id, spec::ACTION_SEND_CLIPBOARD);
    let stop_run = take(&mut by_id, spec::ACTION_STOP_RUN);
    session_menu.append(&focus_input)?;
    session_menu.append(&send_input)?;
    session_menu.append(&send_clipboard)?;
    session_menu.append(&stop_run)?;

    let menu = match Menu::default(app) {
        Ok(menu) => menu,
        Err(error) => {
            tracing::warn!("default menu unavailable, starting a fresh one: {error}");
            Menu::new(app)?
        }
    };
    menu.append(&session_menu)?;
    app.set_menu(menu)?;

    app.manage(SessionMenuStateHandle(std::sync::Mutex::new(Some(
        SessionMenuHandles {
            new_session,
            focus_input,
            send_input,
            send_clipboard,
            stop_run,
            switch_items,
            switch_sessions: vec![None; SWITCH_SLOTS],
            permission_items,
        },
    ))));
    Ok(())
}

/// 把菜单点击翻译成前端事件并派发。
///
/// `session.switch` 需要把槽位下标换成真实会话 id,这一步只有拿着句柄才能做。
pub fn handle_menu_event(app: &AppHandle<Wry>, menu_id: &str) {
    let mut event = match event_for_menu_id(menu_id, None) {
        Some(event) => event,
        None => return,
    };
    if event.action == spec::ACTION_SWITCH_SESSION {
        let index: usize = match spec::dynamic_suffix(menu_id).and_then(|raw| raw.parse().ok()) {
            Some(index) => index,
            None => return,
        };
        let resolved = app
            .try_state::<SessionMenuStateHandle>()
            .and_then(|handle| {
                handle
                    .0
                    .lock()
                    .ok()
                    .and_then(|guard| {
                        guard
                            .as_ref()
                            .and_then(|handles| handles.switch_sessions.get(index).cloned().flatten())
                    })
            });
        match resolved {
            Some(session_id) => event.session_id = Some(session_id),
            // 空槽位不该可点击;真被点到就静默忽略,不发一个空 id 下去。
            None => return,
        }
    }
    if event.action == spec::ACTION_SEND_CLIPBOARD {
        event.text = read_clipboard_text();
    }
    emit(app, event);
}

fn emit(app: &AppHandle<Wry>, event: SessionMenuEvent) {
    if let Err(error) = app.emit(SESSION_MENU_EVENT, &event) {
        tracing::warn!("failed to emit {}: {error}", SESSION_MENU_EVENT);
    }
}

/// 把前端传来的会话列表裁剪到固定槽位数,并算好每项的勾选状态。
///
/// 纯函数:`sync_session_menu` 只是把结果写到原生菜单项上。
pub fn plan_switch_slots(
    entries: &[SessionMenuEntry],
    current: Option<&str>,
) -> Vec<Option<(String, String, bool)>> {
    let mut slots: Vec<Option<(String, String, bool)>> = Vec::with_capacity(SWITCH_SLOTS);
    for index in 0..SWITCH_SLOTS {
        slots.push(entries.get(index).map(|entry| {
            (
                entry.id.clone(),
                entry.label.clone(),
                current.is_some_and(|id| id == entry.id),
            )
        }));
    }
    slots
}

/// 前端同步菜单状态:可用状态与勾选状态必须和界面一致。
///
/// 前端在任何会影响菜单的状态变化(会话列表、当前会话、权限模式、是否在跑)
/// 之后调用一次;原生侧只做改写,不重建菜单树。
#[tauri::command]
pub fn sync_session_menu(
    handle: tauri::State<'_, SessionMenuStateHandle>,
    state: SessionMenuState,
) -> Result<(), String> {
    let mut guard = handle.0.lock().map_err(|_| "menu state poisoned".to_string())?;
    let Some(handles) = guard.as_mut() else {
        // 菜单还没装好(极早期),静默接受:下一次同步会补上。
        return Ok(());
    };

    handles
        .new_session
        .set_enabled(state.has_project)
        .map_err(|error| error.to_string())?;
    handles
        .stop_run
        .set_enabled(state.running)
        .map_err(|error| error.to_string())?;

    let slots = plan_switch_slots(&state.entries, state.current_session_id.as_deref());
    for (index, item) in handles.switch_items.iter().enumerate() {
        let slot = slots.get(index).cloned().flatten();
        let label = slot
            .as_ref()
            .map(|(_, label, _)| label.clone())
            .unwrap_or_else(|| EMPTY_SLOT_LABEL.to_string());
        item.set_text(label).map_err(|error| error.to_string())?;
        item.set_enabled(slot.is_some())
            .map_err(|error| error.to_string())?;
        item.set_checked(slot.as_ref().is_some_and(|(_, _, checked)| *checked))
            .map_err(|error| error.to_string())?;
    }
    for (index, slot) in handles.switch_sessions.iter_mut().enumerate() {
        *slot = slots.get(index).cloned().flatten().map(|(id, _, _)| id);
    }

    for (mode, item) in handles.permission_items.iter() {
        item.set_checked(*mode == state.permission_mode)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 原生侧真读到剪贴板再交给前端,用于「从剪贴板发送到当前会话」。
#[tauri::command]
pub fn read_session_clipboard() -> Option<String> {
    read_clipboard_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_static_menu_item_maps_to_an_event_the_frontend_can_route() {
        for item in spec::ACTION_ITEMS {
            let event = event_for_menu_id(item.id, None)
                .unwrap_or_else(|| panic!("{} 没有派发事件", item.id));
            assert_eq!(event.action, item.id);
        }
    }

    #[test]
    fn clipboard_action_carries_the_native_text_verbatim() {
        let event = event_for_menu_id(spec::ACTION_SEND_CLIPBOARD, Some("你好\n世界".to_string()))
            .expect("剪贴板发送必须派发事件");
        assert_eq!(event.action, spec::ACTION_SEND_CLIPBOARD);
        assert_eq!(event.text.as_deref(), Some("你好\n世界"));
    }

    #[test]
    fn empty_clipboard_still_dispatches_so_the_frontend_validates() {
        let event = event_for_menu_id(spec::ACTION_SEND_CLIPBOARD, None).expect("仍然派发");
        assert_eq!(event.text, None);
    }

    #[test]
    fn permission_items_only_accept_the_three_known_modes() {
        for (mode, _) in spec::PERMISSION_MODES {
            let event = event_for_menu_id(&spec::permission_item_id(mode), None)
                .unwrap_or_else(|| panic!("{mode} 必须可派发"));
            assert_eq!(event.mode.as_deref(), Some(*mode));
        }
        assert!(event_for_menu_id(&spec::permission_item_id("godmode"), None).is_none());
    }

    #[test]
    fn unknown_menu_ids_are_ignored_instead_of_guessed() {
        assert!(event_for_menu_id("system.default.item", None).is_none());
        // 没有下标的切换项不是合法槽位,不能被当成"切换某个不存在的会话"。
        assert!(event_for_menu_id(spec::ACTION_SWITCH_SESSION, None).is_none());
    }

    #[test]
    fn switch_items_dispatch_a_session_switch_action_with_a_placeholder_id() {
        // 下标 → 会话 id 的替换需要菜单句柄,这里只断言动作正确、不会误伤静态项。
        let event = event_for_menu_id(&spec::switch_item_id(4), None).expect("切换项必须派发");
        assert_eq!(event.action, spec::ACTION_SWITCH_SESSION);
        assert_eq!(event.session_id.as_deref(), Some(""));
    }

    fn entry(id: &str, label: &str) -> SessionMenuEntry {
        SessionMenuEntry {
            id: id.to_string(),
            label: label.to_string(),
            checked: false,
        }
    }

    #[test]
    fn switch_slots_only_check_the_current_session_and_leave_the_rest_empty() {
        let entries = vec![
            entry("aaaaaaaa-1", "新会话(9537257c,5 分钟前)"),
            entry("bbbbbbbb-2", "修复菜单(11112222,2 小时前)"),
        ];
        let slots = plan_switch_slots(&entries, Some("bbbbbbbb-2"));
        assert_eq!(slots.len(), SWITCH_SLOTS);
        assert_eq!(
            slots[0],
            Some(("aaaaaaaa-1".to_string(), "新会话(9537257c,5 分钟前)".to_string(), false))
        );
        assert_eq!(
            slots[1],
            Some(("bbbbbbbb-2".to_string(), "修复菜单(11112222,2 小时前)".to_string(), true))
        );
        // 超出列表长度的槽位必须是空的 —— 菜单里不能留下上次的僵尸会话。
        assert!(slots.iter().skip(2).all(|slot| slot.is_none()));
    }

    #[test]
    fn switch_slots_never_exceed_the_menu_capacity() {
        let entries: Vec<SessionMenuEntry> = (0..SWITCH_SLOTS + 12)
            .map(|index| entry(&format!("id-{index}"), &format!("会话 {index}")))
            .collect();
        let slots = plan_switch_slots(&entries, Some("id-0"));
        assert_eq!(slots.len(), SWITCH_SLOTS);
        assert!(slots.iter().all(|slot| slot.is_some()));
    }
}
