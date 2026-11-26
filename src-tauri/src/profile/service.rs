use super::{
    import::{self, ImportItem},
    routing::Routing,
    Node, PublicProfile,
};
use crate::{
    app::{Admission, Kind},
    settings::SettingsStore,
};
use serde::Serialize;
use std::{
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub routing_revision: u64,
    #[serde(flatten)]
    pub profile: PublicProfile,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub ticket: u64,
    pub items: Vec<ImportItem>,
}
struct Pending {
    id: u64,
    created: Instant,
    nodes: Vec<Node>,
}
#[derive(Default)]
struct PreviewState {
    next: u64,
    pending: Option<Pending>,
}
impl PreviewState {
    fn invalidate(&mut self) -> u64 {
        self.next += 1;
        self.pending = None;
        self.next
    }
    fn publish(&mut self, id: u64, nodes: Vec<Node>, cancelled: bool) -> Result<(), String> {
        if self.next != id || cancelled {
            return Err("导入预览已取消，请重新预览".into());
        }
        self.pending = Some(Pending {
            id,
            created: Instant::now(),
            nodes,
        });
        Ok(())
    }
}
pub struct Service {
    store: Arc<SettingsStore>,
    gate: Arc<Admission>,
    preview: Mutex<PreviewState>,
    runtime: Arc<crate::runtime::RuntimeService>,
}
impl Service {
    pub fn new(
        store: Arc<SettingsStore>,
        gate: Arc<Admission>,
        runtime: Arc<crate::runtime::RuntimeService>,
    ) -> Self {
        Self {
            store,
            gate,
            preview: Mutex::new(PreviewState::default()),
            runtime,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let s = self.store.snapshot();
        Snapshot {
            revision: s.nodes_revision,
            routing_revision: s.routing_revision,
            profile: s.profile.public(),
        }
    }
    pub fn preview(&self, input: &str) -> Result<Preview, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        let id = self.preview.lock().unwrap().invalidate();
        let settings = self.store.snapshot();
        if self.store.error().is_some() {
            return Err("设置无法读取，请先处理存储错误".into());
        }
        if input.len() > import::MAX_BYTES {
            return Err("导入内容超过 2 MiB".into());
        }
        let formatted;
        let input = if (input.trim_start().starts_with('/')
            || input.trim_start().starts_with('{')
            || input.trim_start().starts_with('['))
            && serde_json::from_str::<serde_json::Value>(input).is_err()
        {
            let directory = self.store.root.join("import");
            crate::private_files::directory(&directory)?;
            let source = directory.join("source.jsonc");
            crate::private_files::write(&source, input.as_bytes())?;
            struct Cleanup(std::path::PathBuf);
            impl Drop for Cleanup {
                fn drop(&mut self) {
                    let _ = std::fs::remove_file(&self.0);
                }
            }
            let _cleanup = Cleanup(source.clone());
            let core = settings
                .selected_path(&self.store.root)
                .map_err(|_| "JSONC 提取需要等待核心就绪")?;
            formatted = self
                .runtime
                .format_config(&core, &source, &_op.cancel)
                .map_err(|_| "核心未能读取此 JSONC；原文件未被修改，导入尚未提交")?;
            formatted.as_str()
        } else {
            input
        };
        let preview = import::preview(input, &settings.profile.nodes)?;
        let nodes = preview
            .nodes
            .into_iter()
            .enumerate()
            .map(|(index, mut n)| {
                n.id = format!("node-{}-{id}-{index}", crate::logs::now());
                n
            })
            .collect();
        self.preview
            .lock()
            .unwrap()
            .publish(id, nodes, _op.cancel.load(Ordering::Acquire))?;
        Ok(Preview {
            ticket: id,
            items: preview.items,
        })
    }
    pub fn cancel(&self) {
        self.preview.lock().unwrap().invalidate();
    }
    pub fn commit(&self, id: u64, expected: u64) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        let mut state = self.preview.lock().unwrap();
        if state
            .pending
            .as_ref()
            .is_some_and(|p| p.created.elapsed() >= Duration::from_secs(600))
        {
            state.pending = None;
        }
        let p = state
            .pending
            .as_ref()
            .filter(|p| p.id == id)
            .ok_or("导入预览已过期，请重新预览")?;
        let nodes = p.nodes.clone();
        self.store.profile(expected, |profile| {
            for node in nodes {
                if !profile
                    .nodes
                    .iter()
                    .any(|old| old.spec.fingerprint() == node.spec.fingerprint())
                {
                    profile.nodes.push(node);
                }
            }
            Ok(())
        })?;
        state.pending = None;
        Ok(self.snapshot())
    }
    pub fn rename(
        &self,
        id: String,
        name: String,
        tags: Vec<String>,
        expected: u64,
    ) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        let name = name.trim();
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err("节点名称为空、过长或包含控制字符".into());
        }
        let tags = super::normalize_tags(tags)?;
        self.store.profile(expected, |p| {
            let node = p
                .nodes
                .iter_mut()
                .find(|n| n.id == id)
                .ok_or("节点不存在")?;
            node.name = name.into();
            node.tags = tags;
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    pub fn delete(&self, id: String, expected: u64) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Selection)?;
        self.runtime.stopped()?;
        self.store.profile(expected, |p| {
            if p.nodes.iter().all(|n| n.id != id) {
                return Err("节点不存在".into());
            }
            p.nodes.retain(|n| n.id != id);
            if p.selected_node.as_ref() == Some(&id) {
                p.selected_node = None;
            }
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    pub fn routing(&self, routing: Routing, expected: u64) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        self.store.routing(expected, routing)?;
        Ok(self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_rejects_a_preview_that_finishes_after_cancel() {
        let mut state = PreviewState::default();
        let old = state.invalidate();
        state.invalidate();
        assert!(state.publish(old, vec![], false).is_err());
        assert!(state.pending.is_none());
        let current = state.invalidate();
        assert!(state.publish(current, vec![], true).is_err());
        assert!(state.pending.is_none());
        state.publish(current, vec![], false).unwrap();
        assert_eq!(state.pending.as_ref().unwrap().id, current);
        assert!(state.publish(old, vec![], false).is_err());
        assert_eq!(state.pending.as_ref().unwrap().id, current);
        state.invalidate();
        assert!(state.pending.is_none());
    }
    #[test]
    fn expired_preview_is_destroyed_without_writing_settings() {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-expired-import-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        let store = Arc::new(SettingsStore::open(root.clone()).unwrap());
        let service = Service::new(
            store.clone(),
            Arc::new(Admission::default()),
            Arc::new(Default::default()),
        );
        let preview = service.preview("trojan://test@example.test:443").unwrap();
        service
            .preview
            .lock()
            .unwrap()
            .pending
            .as_mut()
            .unwrap()
            .created = Instant::now() - Duration::from_secs(600);
        assert!(service.commit(preview.ticket, 0).is_err());
        assert!(service.preview.lock().unwrap().pending.is_none());
        assert_eq!(store.snapshot().nodes_revision, 0);
        drop(service);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
