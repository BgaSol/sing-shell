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
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
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
pub struct Service {
    store: Arc<SettingsStore>,
    gate: Arc<Admission>,
    pending: Mutex<Option<Pending>>,
    next: AtomicU64,
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
            pending: Mutex::new(None),
            next: AtomicU64::new(0),
            runtime,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let s = self.store.snapshot();
        Snapshot {
            revision: s.profile_revision,
            profile: s.profile.public(),
        }
    }
    pub fn preview(&self, input: &str) -> Result<Preview, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        let settings = self.store.snapshot();
        if self.store.error.is_some() {
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
        let id = self.next.fetch_add(1, Ordering::AcqRel) + 1;
        let nodes = preview
            .nodes
            .into_iter()
            .enumerate()
            .map(|(index, mut n)| {
                n.id = format!("node-{}-{id}-{index}", crate::logs::now());
                n
            })
            .collect();
        *self.pending.lock().unwrap() = Some(Pending {
            id,
            created: Instant::now(),
            nodes,
        });
        Ok(Preview {
            ticket: id,
            items: preview.items,
        })
    }
    pub fn cancel(&self) {
        self.pending.lock().unwrap().take();
    }
    pub fn commit(&self, id: u64, expected: u64) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        let mut pending = self.pending.lock().unwrap();
        let p = pending
            .as_ref()
            .filter(|p| p.id == id && p.created.elapsed() < Duration::from_secs(600))
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
        pending.take();
        Ok(self.snapshot())
    }
    pub fn select(&self, id: String, expected: u64) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        self.store.profile(expected, |p| {
            if p.nodes.iter().all(|n| n.id != id) {
                return Err("节点不存在".into());
            }
            p.selected_node = Some(id);
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    pub fn rename(&self, id: String, name: String, expected: u64) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Profile)?;
        let name = name.trim();
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Err("节点名称为空、过长或包含控制字符".into());
        }
        self.store.profile(expected, |p| {
            p.nodes
                .iter_mut()
                .find(|n| n.id == id)
                .ok_or("节点不存在")?
                .name = name.into();
            Ok(())
        })?;
        Ok(self.snapshot())
    }
    pub fn delete(&self, id: String, expected: u64) -> Result<Snapshot, String> {
        let _op = self.gate.begin(Kind::Profile)?;
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
        self.store.profile(expected, |p| {
            p.routing = routing;
            Ok(())
        })?;
        Ok(self.snapshot())
    }
}
