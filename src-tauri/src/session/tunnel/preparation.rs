use super::ConnectionInput;
use crate::{activity::Context, platform::network, runtime::RuntimeService};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, Mutex},
};
#[derive(Clone)]
pub(super) struct RunningConnection {
    pub(super) input: ConnectionInput,
    pub(super) bypass: crate::profile::bypass::Resolved,
}
impl std::ops::Deref for RunningConnection {
    type Target = ConnectionInput;
    fn deref(&self) -> &Self::Target {
        &self.input
    }
}
pub(super) struct Prepared {
    pub(super) running: RunningConnection,
    pub(super) executable: PathBuf,
    pub(super) config: PathBuf,
    pub(super) directory: PathBuf,
    pub(super) context: Option<Context>,
    pub(super) session_id: String,
}

pub(super) fn prepare(
    root: &Path,
    owner_key: &str,
    input: ConnectionInput,
    frozen: Option<crate::profile::bypass::Resolved>,
    cancel: &Arc<AtomicBool>,
    preparation: &Mutex<super::resolver::Resources>,
) -> Result<Prepared, String> {
    input.validate()?;
    network::require_fakeip_clear()?;
    let mut keep = input.versions.clone();
    if let Ok(bytes) = std::fs::read(root.join("rules/pinned.json")) {
        if let Ok(pinned) = serde_json::from_slice::<Vec<crate::rulesets::Version>>(&bytes) {
            if pinned.iter().all(|v| v.validate().is_ok()) {
                keep.extend(pinned);
            }
        }
    }
    crate::rulesets::prune(root, &keep)?;
    let check = RuntimeService::default();
    let executable = super::install::existing(root, &input.core_version, &check, cancel)?;
    for expected in &input.versions {
        let mut preferences = crate::updates::network::Preferences::default();
        preferences.fallback_enabled = false;
        let v = crate::rulesets::prepare(
            root,
            &expected.source_id,
            &expected.commit,
            preferences,
            cancel,
        )?;
        if &v != expected {
            return Err("受保护规则与固定版本不一致".into());
        }
    }
    let context = if check.supports_activity(&executable, cancel)? {
        Some(Context::new()?)
    } else {
        None
    };
    let session_id = context
        .as_ref()
        .map(|c| Ok(c.id.clone()))
        .unwrap_or_else(crate::platform::authorization::nonce)?;
    let directory = root.join("runtime").join(&session_id);
    preparation
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .create(root, &session_id)?;
    let config = directory.join("config.json");
    let bypass = match frozen {
        Some(b) => {
            b.validate(&input.routing)?;
            b
        }
        None => {
            if !crate::profile::bypass::Targets::collect(&input.routing)?
                .domains
                .is_empty()
                && !check.supports_activity(&executable, cancel)?
            {
                return Err("当前核心不支持域名绕过所需的 DNS 查询接口".into());
            }
            super::resolver::resolve(
                root,
                &executable,
                &input,
                &mut preparation.lock().unwrap_or_else(|p| p.into_inner()),
                cancel,
            )?
        }
    };
    let result = (|| {
        let cache = super::cache::prepare(root, owner_key)?;
        let mut bindings = crate::profile::config::Bindings::prepare(root, &input.versions, cache)?;
        let mut secrets = input.node.spec.secrets();
        if let Some(context) = &context {
            bindings.activity = Some((context.port, context.secret.clone()));
            secrets.push(context.secret.clone());
            secrets.push(format!("127.0.0.1:{}", context.port));
        }
        check.logs.redact(secrets);
        let value = crate::profile::config::compile_runtime(&input, &bindings, &bypass)?;
        crate::settings::write_json(&config, &value)?;
        check.validate(&executable, &config, cancel)
    })();
    if let Err(e) = result {
        return Err(e);
    }
    Ok(Prepared {
        running: RunningConnection { input, bypass },
        executable,
        config,
        directory,
        context,
        session_id,
    })
}
