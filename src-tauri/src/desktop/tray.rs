use crate::{
    app::Application,
    lifecycle::{self, Target},
    profile::Mode,
    session::{Action, Restore},
};
use std::{collections::BTreeMap, sync::Arc};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    Manager,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum IconState {
    Idle,
    Busy,
    Running,
    Attention,
}
#[derive(Clone, Debug, PartialEq)]
struct NodeItem {
    id: String,
    label: String,
}
#[derive(Clone, Debug, PartialEq)]
struct ActionItem {
    id: String,
    label: String,
    enabled: bool,
}
#[derive(Clone, Debug, PartialEq)]
struct Projection {
    status: String,
    icon: IconState,
    generated: bool,
    revision: u64,
    nodes: Vec<NodeItem>,
    selected: Option<String>,
    selection_label: String,
    selection_enabled: bool,
    actions: Vec<ActionItem>,
}
// Native menus interpret ampersands and control characters as menu syntax.
fn literal(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .replace('&', "&&")
}
impl Projection {
    fn new(
        settings: &crate::settings::Settings,
        s: &crate::session::Snapshot,
        storage_error: bool,
    ) -> Self {
        let generated = s.mode == Mode::Generated;
        let issue = s.runtime.error.is_some()
            || s.core.config_error.is_some()
            || s.core.maintenance.error.is_some()
            || storage_error;
        let running = s.phase == "running" && s.runtime.pid.is_some();
        let selected = settings.profile.selected();
        let name = s
            .running_node
            .as_ref()
            .map(|n| n.name.as_str())
            .unwrap_or("当前服务器");
        let (status, icon) = if s.runtime.cleanup_pending {
            ("网络资源待清理".into(), IconState::Attention)
        } else if s.phase == "stopping" {
            ("正在断开…".into(), IconState::Busy)
        } else if let Some(target) = &s.target {
            (
                if running {
                    format!("正在切换到 {} · 当前 {}", target.name, name)
                } else {
                    format!("正在连接：{}", target.name)
                },
                IconState::Busy,
            )
        } else if s.phase == "authorizing" {
            ("正在请求网络授权".into(), IconState::Busy)
        } else if matches!(s.phase.as_str(), "preparing" | "starting") || s.operation.is_some() {
            ("正在准备连接…".into(), IconState::Busy)
        } else if running {
            (
                if generated {
                    format!("已连接：{name}")
                } else {
                    "核心运行中 · 原始配置模式".into()
                },
                IconState::Running,
            )
        } else if s.restore == Restore::Waiting {
            ("等待核心恢复".into(), IconState::Busy)
        } else if !s.core.initialized {
            ("正在初始化…".into(), IconState::Busy)
        } else {
            (
                if generated {
                    "未连接".into()
                } else {
                    "核心已停止 · 原始配置模式".into()
                },
                if issue {
                    IconState::Attention
                } else {
                    IconState::Idle
                },
            )
        };
        let mut counts = BTreeMap::new();
        for node in &settings.profile.nodes {
            *counts.entry(node.name.as_str()).or_insert(0) += 1;
        }
        let nodes = settings
            .profile
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let summary = node.summary();
                let label = if counts[node.name.as_str()] > 1 {
                    format!(
                        "{} · {} {}:{} · {}",
                        node.name,
                        summary.protocol,
                        summary.server,
                        summary.port,
                        index + 1
                    )
                } else {
                    node.name.clone()
                };
                NodeItem {
                    id: node.id.clone(),
                    label: literal(&label),
                }
            })
            .collect();
        let mut actions = Vec::new();
        if generated {
            if s.target.is_none()
                && s.operation.is_none()
                && s.phase != "stopping"
                && (!running || selected.map(|n| &n.id) != s.running_node.as_ref().map(|n| &n.id))
            {
                actions.push(ActionItem {
                    id: selected
                        .map(|n| format!("connect:{}", n.id))
                        .unwrap_or("connect-none".into()),
                    label: selected
                        .map(|n| {
                            if running {
                                literal(&format!("切换到 {}", n.name))
                            } else {
                                "连接".into()
                            }
                        })
                        .unwrap_or("连接".into()),
                    enabled: s.can_connect && selected.is_some(),
                });
            }
        } else if !s.can_stop && s.phase != "stopping" {
            actions.push(ActionItem {
                id: "raw-start".into(),
                label: "启动核心".into(),
                enabled: s.can_start,
            });
        }
        if s.can_stop || s.phase == "stopping" {
            actions.push(ActionItem {
                id: "stop".into(),
                label: if s.runtime.cleanup_pending {
                    "重试断开"
                } else if s.phase == "stopping" {
                    "正在断开…"
                } else if generated && !running && s.target.is_some() {
                    "取消连接"
                } else if generated {
                    "断开连接"
                } else {
                    "停止核心"
                }
                .into(),
                enabled: s.can_stop,
            });
        }
        if issue {
            actions.push(ActionItem {
                id: "problem".into(),
                label: "查看原因".into(),
                enabled: true,
            });
        }
        Self {
            status: if issue && running && s.target.is_none() {
                format!("{status} · 有问题待处理")
            } else {
                status
            },
            icon,
            generated,
            revision: settings.nodes_revision,
            nodes,
            selected: selected.map(|n| n.id.clone()),
            selection_label: literal(&format!(
                "服务器：{}",
                selected.map(|n| n.name.as_str()).unwrap_or("请选择服务器")
            )),
            selection_enabled: s.can_select_node || settings.profile.nodes.is_empty(),
            actions,
        }
    }
    fn tooltip(&self) -> String {
        format!("Sing Shell · {}", self.status)
    }
}

pub(super) struct Tray {
    icon: TrayIcon,
    menu: Menu<tauri::Wry>,
    status: MenuItem<tauri::Wry>,
    servers: Submenu<tauri::Wry>,
    nodes: Vec<CheckMenuItem<tauri::Wry>>,
    actions: Vec<MenuItem<tauri::Wry>>,
    projection: Projection,
}
impl Tray {
    pub(super) fn install(handle: &tauri::AppHandle) -> Result<Self, String> {
        let app = handle.state::<Arc<Application>>();
        let settings = app.store.snapshot();
        let projection = Projection::new(
            &settings,
            &app.session.snapshot_for(&settings),
            app.store.error().is_some(),
        );
        let status = MenuItem::with_id(
            handle,
            "status",
            literal(&projection.status),
            true,
            None::<&str>,
        )
        .map_err(|e| e.to_string())?;
        let servers = Submenu::new(
            handle,
            &projection.selection_label,
            projection.selection_enabled,
        )
        .map_err(|e| e.to_string())?;
        let menu = Menu::new(handle).map_err(|e| e.to_string())?;
        menu.append(&status).map_err(|e| e.to_string())?;
        menu.append(&PredefinedMenuItem::separator(handle).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if projection.generated {
            menu.append(&servers).map_err(|e| e.to_string())?;
        }
        let actions = Self::make_actions(handle, &menu, &projection)?;
        menu.append(&PredefinedMenuItem::separator(handle).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for (id, label) in [
            ("show", "打开主窗口"),
            ("settings", "设置"),
            ("quit", "退出"),
        ] {
            menu.append(
                &MenuItem::with_id(
                    handle,
                    id,
                    label,
                    true,
                    if id == "quit" {
                        Some("CmdOrCtrl+Q")
                    } else {
                        None
                    },
                )
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        let icon = TrayIconBuilder::with_id("sing-shell")
            .icon(icon(projection.icon))
            .icon_as_template(cfg!(target_os = "macos"))
            .tooltip(projection.tooltip())
            .menu(&menu)
            .show_menu_on_left_click(!cfg!(windows))
            .on_menu_event(|handle, event| {
                let app = handle.state::<Arc<Application>>();
                let id = event.id.as_ref();
                let result = match id {
                    "quit" => lifecycle::request(app.inner().clone(), handle.clone(), Target::Quit),
                    "show" | "nodes" => app.desktop.navigate(handle, "nodes"),
                    "settings" => app.desktop.navigate(handle, "settings"),
                    "status" | "problem" => app.desktop.navigate(handle, "activity"),
                    "stop" => app.session.action(Action::Stop).map(|_| ()),
                    "raw-start" => app.session.start_external().map(|_| ()),
                    _ => {
                        if let Some(node) = id.strip_prefix("connect:") {
                            app.session.connect(node.into()).map(|_| ())
                        } else if let Some(node) = id.strip_prefix("select:") {
                            let revision = {
                                let state = app.desktop.state.lock().unwrap();
                                state.tray.as_ref().map(|tray| {
                                    if let Some(index) =
                                        tray.projection.nodes.iter().position(|n| n.id == node)
                                    {
                                        let _ = tray.nodes[index].set_checked(
                                            tray.projection.selected.as_deref() == Some(node),
                                        );
                                    }
                                    tray.projection.revision
                                })
                            };
                            revision
                                .ok_or("托盘已不可用".into())
                                .and_then(|revision| app.session.select_node(node.into(), revision))
                        } else {
                            Ok(())
                        }
                    }
                };
                if let Err(error) = result {
                    app.runtime.report(error);
                }
                if let Err(error) = app.desktop.refresh(&app) {
                    app.desktop.tray_failed(handle, error);
                }
            })
            .on_tray_icon_event(|tray, event| {
                if cfg!(windows)
                    && matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    )
                {
                    let handle = tray.app_handle();
                    let app = handle.state::<Arc<Application>>();
                    if let Err(error) = app.desktop.present(handle, true) {
                        app.desktop.report(handle, error);
                    }
                }
            })
            .build(handle)
            .map_err(|e| e.to_string())?;
        let mut tray = Self {
            icon,
            menu,
            status,
            servers,
            nodes: vec![],
            actions,
            projection,
        };
        tray.rebuild_nodes(handle)?;
        Ok(tray)
    }
    fn make_actions(
        handle: &tauri::AppHandle,
        menu: &Menu<tauri::Wry>,
        p: &Projection,
    ) -> Result<Vec<MenuItem<tauri::Wry>>, String> {
        p.actions
            .iter()
            .enumerate()
            .map(|(i, action)| {
                let item = MenuItem::with_id(
                    handle,
                    &action.id,
                    &action.label,
                    action.enabled,
                    None::<&str>,
                )
                .map_err(|e| e.to_string())?;
                menu.insert(&item, 2 + usize::from(p.generated) + i)
                    .map_err(|e| e.to_string())?;
                Ok(item)
            })
            .collect()
    }
    fn rebuild_nodes(&mut self, handle: &tauri::AppHandle) -> Result<(), String> {
        for item in self.servers.items().map_err(|e| e.to_string())? {
            self.servers.remove(&item).map_err(|e| e.to_string())?;
        }
        self.nodes.clear();
        for node in &self.projection.nodes {
            let item = CheckMenuItem::with_id(
                handle,
                format!("select:{}", node.id),
                &node.label,
                true,
                self.projection.selected.as_ref() == Some(&node.id),
                None::<&str>,
            )
            .map_err(|e| e.to_string())?;
            self.servers.append(&item).map_err(|e| e.to_string())?;
            self.nodes.push(item);
        }
        if self.nodes.is_empty() {
            self.servers
                .append(
                    &MenuItem::with_id(handle, "nodes", "打开主窗口添加服务器", true, None::<&str>)
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub(super) fn refresh(&mut self, app: &Application) -> Result<(), String> {
        let settings = app.store.snapshot();
        let next = Projection::new(
            &settings,
            &app.session.snapshot_for(&settings),
            app.store.error().is_some(),
        );
        self.update(next)
    }
    fn update(&mut self, next: Projection) -> Result<(), String> {
        if self.projection == next {
            return Ok(());
        }
        let handle = self.icon.app_handle().clone();
        if self.projection.status != next.status {
            self.status
                .set_text(literal(&next.status))
                .map_err(|e| e.to_string())?;
            self.icon
                .set_tooltip(Some(next.tooltip()))
                .map_err(|e| e.to_string())?;
        }
        if self.projection.icon != next.icon {
            self.icon
                .set_icon_with_as_template(Some(icon(next.icon)), cfg!(target_os = "macos"))
                .map_err(|e| e.to_string())?;
        }
        let mode_changed = self.projection.generated != next.generated;
        if mode_changed {
            if next.generated {
                self.menu
                    .insert(&self.servers, 2)
                    .map_err(|e| e.to_string())?;
            } else {
                self.menu.remove(&self.servers).map_err(|e| e.to_string())?;
            }
        }
        if self
            .projection
            .actions
            .iter()
            .map(|a| &a.id)
            .ne(next.actions.iter().map(|a| &a.id))
            || mode_changed
        {
            for action in &self.actions {
                self.menu.remove(action).map_err(|e| e.to_string())?;
            }
            self.actions = Self::make_actions(&handle, &self.menu, &next)?;
        } else {
            for ((item, old), new) in self
                .actions
                .iter()
                .zip(&self.projection.actions)
                .zip(&next.actions)
            {
                if old.label != new.label {
                    item.set_text(&new.label).map_err(|e| e.to_string())?;
                }
                if old.enabled != new.enabled {
                    item.set_enabled(new.enabled).map_err(|e| e.to_string())?;
                }
            }
        }
        if self.projection.selection_label != next.selection_label {
            self.servers
                .set_text(&next.selection_label)
                .map_err(|e| e.to_string())?;
        }
        if self.projection.selection_enabled != next.selection_enabled {
            self.servers
                .set_enabled(next.selection_enabled)
                .map_err(|e| e.to_string())?;
        }
        let rebuild = self.projection.nodes != next.nodes;
        if !rebuild && self.projection.selected != next.selected {
            for (item, node) in self.nodes.iter().zip(&next.nodes) {
                if self.projection.selected.as_ref() == Some(&node.id)
                    || next.selected.as_ref() == Some(&node.id)
                {
                    item.set_checked(next.selected.as_ref() == Some(&node.id))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        self.projection = next;
        if rebuild {
            self.rebuild_nodes(&handle)?;
        }
        Ok(())
    }
    pub(super) fn verify_native(&mut self) -> Result<serde_json::Value, String> {
        // Isolated smoke only: exercise real menu handles without saving nodes or starting a core.
        let original = self.projection.clone();
        let started = std::time::Instant::now();
        let result = (|| {
            let mut next = original.clone();
            next.generated = true;
            next.selection_enabled = true;
            next.selected = Some("smoke-0".into());
            next.nodes = (0..2000)
                .map(|i| NodeItem {
                    id: format!("smoke-{i}"),
                    label: literal(&format!("测试服务器 & {i} · 长名称 日本 香港 新加坡")),
                })
                .collect();
            next.actions = vec![ActionItem {
                id: "connect:smoke-0".into(),
                label: "连接".into(),
                enabled: true,
            }];
            next.selection_label = "服务器：测试服务器 0".into();
            self.update(next.clone())?;
            if self.servers.items().map_err(|e| e.to_string())?.len() != 2000
                || !self.nodes[0].is_checked().map_err(|e| e.to_string())?
            {
                return Err("原生托盘节点列表或勾选不完整".into());
            }
            let id = self.nodes[1999].id().clone();
            next.selected = Some("smoke-1999".into());
            next.selection_label = "服务器：测试服务器 1999".into();
            next.actions[0].id = "connect:smoke-1999".into();
            self.update(next.clone())?;
            if self.nodes[0].is_checked().map_err(|e| e.to_string())?
                || !self.nodes[1999].is_checked().map_err(|e| e.to_string())?
                || self.nodes[1999].id() != &id
            {
                return Err("原生托盘改选未保留菜单与唯一勾选".into());
            }
            next.nodes[1999].label = literal("重命名 & 服务器");
            self.update(next.clone())?;
            // AppKit returns the displayed title, whereas Windows/GTK preserve mnemonic escapes.
            let expected = if cfg!(target_os = "macos") {
                next.nodes[1999].label.replace("&&", "&")
            } else {
                next.nodes[1999].label.clone()
            };
            if self.nodes[1999].text().map_err(|e| e.to_string())? != expected {
                return Err("原生托盘名称未同步".into());
            }
            next.generated = false;
            self.update(next)?;
            if self
                .menu
                .items()
                .map_err(|e| e.to_string())?
                .iter()
                .any(|item| item.id() == self.servers.id())
            {
                return Err("原始配置模式仍保留服务器菜单".into());
            }
            Ok(
                serde_json::json!({"nodes":2000,"selectionUpdated":true,"renameUpdated":true,"modeUpdated":true,"elapsedMs":started.elapsed().as_millis()}),
            )
        })();
        self.update(original)?;
        result
    }
}

fn icon(state: IconState) -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    let image = match state {
        IconState::Idle => tauri::include_image!("icons/tray-template-idle@2x.png"),
        IconState::Busy => tauri::include_image!("icons/tray-template-busy@2x.png"),
        IconState::Running => tauri::include_image!("icons/tray-template-running@2x.png"),
        IconState::Attention => tauri::include_image!("icons/tray-template-attention@2x.png"),
    };
    #[cfg(not(target_os = "macos"))]
    let image = match state {
        IconState::Idle => tauri::include_image!("icons/tray-idle.png"),
        IconState::Busy => tauri::include_image!("icons/tray-busy.png"),
        IconState::Running => tauri::include_image!("icons/tray-running.png"),
        IconState::Attention => tauri::include_image!("icons/tray-attention.png"),
    };
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_separates_selection_actual_target_and_cleanup() {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-tray-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        let app = Application::open(root.clone(), "0.0.0".into()).unwrap();
        let p = app
            .profiles
            .preview("trojan://secret@example.test:443#A\ntrojan://other@example.test:444#B")
            .unwrap();
        app.profiles.commit(p.ticket, 0).unwrap();
        let mut settings = app.store.snapshot();
        let a = settings.profile.nodes[0].summary();
        let b = settings.profile.nodes[1].summary();
        settings.profile.selected_node = Some(b.id.clone());
        let mut s = app.session.snapshot_for(&settings);
        s.core.initialized = true;
        s.phase = "running".into();
        s.runtime.pid = Some(42);
        s.running_node = Some(a.clone());
        s.can_stop = true;
        s.can_connect = true;
        s.can_select_node = true;
        let p = Projection::new(&settings, &s, false);
        assert_eq!(p.status, "已连接：A");
        assert_eq!(p.selection_label, "服务器：B");
        assert_eq!(p.actions[0].id, format!("connect:{}", b.id));
        assert_eq!(p.actions[1].id, "stop");
        s.target = Some(b);
        s.can_connect = false;
        s.can_select_node = false;
        let p = Projection::new(&settings, &s, false);
        assert_eq!(p.status, "正在切换到 B · 当前 A");
        assert_eq!(p.actions.len(), 1);
        assert_eq!(p.actions[0].id, "stop");
        s.target = None;
        s.core.maintenance.error = Some("maintenance".into());
        assert_eq!(
            Projection::new(&settings, &s, false).icon,
            IconState::Running
        );
        s.runtime.cleanup_pending = true;
        assert_eq!(
            Projection::new(&settings, &s, false).icon,
            IconState::Attention
        );
        s.runtime.cleanup_pending = false;
        s.phase = "stopping".into();
        s.can_stop = false;
        let p = Projection::new(&settings, &s, false);
        assert_eq!(p.actions[0].id, "stop");
        assert!(!p.actions[0].enabled);
        s.phase = "starting".into();
        assert_eq!(Projection::new(&settings, &s, false).icon, IconState::Busy);
        for state in [
            IconState::Idle,
            IconState::Busy,
            IconState::Running,
            IconState::Attention,
        ] {
            let image = icon(state);
            assert_eq!(
                image.rgba().len(),
                image.width() as usize * image.height() as usize * 4
            );
        }
        drop(app);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn menu_text_is_literal_and_duplicate_nodes_keep_distinct_ids() {
        assert_eq!(literal("A&B\t\nC"), "A&&BC");
        let mut settings = crate::settings::Settings::default();
        let mut node = crate::profile::import::preview("trojan://secret@example.test:443#A&B", &[])
            .unwrap()
            .nodes
            .remove(0);
        node.id = "first".into();
        settings.profile.nodes.push(node.clone());
        node.id = "second".into();
        settings.profile.nodes.push(node);
        let root = std::env::temp_dir().join(format!(
            "sing-shell-tray-names-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        let app = Application::open(root.clone(), "0.0.0".into()).unwrap();
        let p = Projection::new(&settings, &app.session.snapshot_for(&settings), false);
        assert_ne!(p.nodes[0].label, p.nodes[1].label);
        assert_ne!(p.nodes[0].id, p.nodes[1].id);
        assert!(!format!("{p:?}").contains("secret"));
        drop(app);
        let _ = std::fs::remove_dir_all(root);
    }
}
