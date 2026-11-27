use crate::{
    logs::Logs,
    platform::process::record::{self, Kind},
    profile::{
        bypass::{Resolved, Targets},
        ConnectionInput,
    },
    runtime::{
        control::{self, Endpoint},
        process::Process,
    },
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct Resources {
    pub directory: Option<PathBuf>,
    process: Option<Process>,
}
impl Resources {
    pub fn create(&mut self, root: &Path, id: &str) -> Result<PathBuf, String> {
        if self.directory.is_some() || self.process.is_some() {
            return Err("上一准备资源尚未清理".into());
        }
        let directory = root.join("runtime").join(id);
        crate::private_files::directory(&directory)?;
        self.directory = Some(directory.clone());
        crate::settings::write_json(&root.join("runtime/preparing.json"), &id)?;
        Ok(directory)
    }
    pub fn close_process(&mut self, root: &Path) -> Result<(), String> {
        if let Some(process) = &mut self.process {
            process.terminate()?;
        }
        self.process = None;
        record::recover(root, Kind::Resolver)?;
        record::forget(root, Kind::Resolver)
    }
    pub fn release(&mut self, root: &Path) -> Result<(), String> {
        self.close_process(root)?;
        remove_record(root)?;
        self.directory = None;
        Ok(())
    }
    pub fn close(&mut self, root: &Path) -> Result<(), String> {
        self.close_process(root)?;
        if let Some(directory) = &self.directory {
            if directory.parent() != Some(root.join("runtime").as_path()) {
                return Err("准备目录超出受保护范围".into());
            }
            match std::fs::remove_dir_all(directory) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        remove_record(root)?;
        self.directory = None;
        Ok(())
    }
    pub fn recover(root: &Path) -> Result<(), String> {
        record::recover(root, Kind::Resolver)?;
        let marker = root.join("runtime/preparing.json");
        let bytes = match std::fs::read(&marker) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        let id: String = serde_json::from_slice(&bytes).map_err(|_| "准备目录记录无效")?;
        if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("准备目录记录无效".into());
        }
        Self {
            directory: Some(root.join("runtime").join(id)),
            process: None,
        }
        .close(root)
    }
}
fn remove_record(root: &Path) -> Result<(), String> {
    match std::fs::remove_file(root.join("runtime/preparing.json")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub(super) fn resolve(
    root: &Path,
    core: &Path,
    input: &ConnectionInput,
    resources: &mut Resources,
    cancel: &Arc<AtomicBool>,
) -> Result<Resolved, String> {
    let targets = Targets::collect(&input.routing)?;
    if targets.domains.is_empty() {
        return Resolved::new(&input.routing, BTreeMap::new());
    }
    resolve_with_server(
        root,
        core,
        input,
        resources,
        cancel,
        crate::platform::dns::upstream::resolver()?,
    )
}
fn resolve_with_server(
    root: &Path,
    core: &Path,
    input: &ConnectionInput,
    resources: &mut Resources,
    cancel: &Arc<AtomicBool>,
    server: serde_json::Value,
) -> Result<Resolved, String> {
    let targets = Targets::collect(&input.routing)?;
    let endpoint = Endpoint::new()?;
    let directory = resources.directory.as_ref().ok_or("准备目录不存在")?;
    let config = directory.join("resolver.json");
    let value = serde_json::json!({"log":{"level":"info"},"dns":{"servers":[server],"final":"physical","disable_cache":true},"route":{"auto_detect_interface":true},"experimental":{"clash_api":endpoint.config()}});
    crate::settings::write_json(&config, &value)?;
    let logs = Logs::default();
    logs.redact(vec![endpoint.secret.clone()]);
    let mut command = Command::new(core);
    command
        .args(["run", "-c"])
        .arg(config)
        .current_dir(directory);
    let process = Process::spawn(&mut command, &logs, None)?;
    resources.process = Some(process);
    record::record(
        root,
        Kind::Resolver,
        resources.process.as_ref().unwrap().child.id(),
        core,
    )?;
    // Trust readiness from this child's output, not an arbitrary process on the chosen port.
    let ready = Instant::now() + Duration::from_secs(5);
    let listening = format!("restful api listening at 127.0.0.1:{}", endpoint.port);
    loop {
        crate::core::package::cancelled(cancel)?;
        if resources
            .process
            .as_mut()
            .unwrap()
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err("域名解析核心启动失败，请重试连接".into());
        }
        if logs
            .snapshot()
            .iter()
            .any(|line| line.message.contains(&listening))
        {
            break;
        }
        if Instant::now() >= ready {
            return Err("域名解析接口未能就绪，请重试连接".into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let client = control::client(Duration::from_secs(5))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut hosts = BTreeMap::new();
    for (domain, rule) in targets.domains {
        let addresses = loop {
            crate::core::package::cancelled(cancel)?;
            if resources
                .process
                .as_mut()
                .unwrap()
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err(format!("{rule}：域名解析核心已退出，绕过 TUN 未应用"));
            }
            if Instant::now() >= deadline {
                return Err(format!("{rule}：{domain} 解析超时，绕过 TUN 未应用"));
            }
            let query=runtime.block_on(async {
                tokio::select! {
                    result=async {tokio::try_join!(endpoint.query(&client,&domain,"A"),endpoint.query(&client,&domain,"AAAA"))}=>result,
                    _=async {while !cancel.load(std::sync::atomic::Ordering::Acquire){tokio::time::sleep(Duration::from_millis(25)).await}}=>Err("连接已取消".into()),
                    _=tokio::time::sleep(deadline.saturating_duration_since(Instant::now()))=>Err("域名解析总时限已到".into()),
                }
            });
            if resources
                .process
                .as_mut()
                .unwrap()
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err("域名解析核心已退出".into());
            }
            match query {
                Ok((mut v4, v6)) => {
                    v4.extend(v6);
                    break v4;
                }
                Err(e)
                    if e == "绕过域名解析接口不可用"
                        && deadline.saturating_duration_since(Instant::now())
                            > Duration::from_secs(29) =>
                {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(e) => return Err(format!("{rule}：无法解析 {domain}，绕过 TUN 未应用：{e}")),
            }
        };
        hosts.insert(domain, addresses);
    }
    let result = Resolved::new(&input.routing, hosts)?;
    resources.close_process(root)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires official core in SING_SHELL_TEST_CORE; isolated DNS, no TUN or system DNS changes"]
    fn official_bypass_resolver_freezes_results_and_releases_process() {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::FreeConsole();
        }
        crate::platform::network_worker().unwrap();
        let root = std::env::temp_dir().join(format!(
            "ss-resolver-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        let core = root.join("engines/sing-box/test").join(if cfg!(windows) {
            "sing-box.exe"
        } else {
            "sing-box"
        });
        std::fs::create_dir_all(core.parent().unwrap()).unwrap();
        std::fs::copy(std::env::var_os("SING_SHELL_TEST_CORE").unwrap(), &core).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&core, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let node = crate::profile::import::preview("trojan://s@example.com:443", &[])
            .unwrap()
            .nodes
            .remove(0);
        let input=ConnectionInput {node,routing:serde_json::from_value(serde_json::json!({"defaultExit":"proxy","policies":[{"id":"rd","name":"RustDesk","enabled":true,"exit":"direct","bypassTun":true,"match":{"kind":"custom","domains":[{"value":"id.example.test","includeSubdomains":false}],"ipCidrs":[]}}]})).unwrap(),versions:vec![],core_version:"1.14.4".into(),options:Default::default()};
        let mut resources = Resources::default();
        resources
            .create(&root, &crate::platform::authorization::nonce().unwrap())
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let result = resolve_with_server(
            &root,
            &core,
            &input,
            &mut resources,
            &cancel,
            serde_json::json!({"type":"hosts","tag":"physical","predefined":{"id.example.test":["192.0.2.7","2001:db8::7"]}}),
        );
        resources.close(&root).unwrap();
        let resolved = result.unwrap();
        assert_eq!(resolved.prefixes.len(), 2);
        assert!(!root.join("preparation-process.json").exists());
        assert!(!root.join("runtime/preparing.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_preparation_record_is_never_followed() {
        let root = std::env::temp_dir().join(format!(
            "ss-bad-prep-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        std::fs::write(root.join("runtime/preparing.json"), br#""../../outside""#).unwrap();
        assert!(Resources::recover(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
