use crate::updates::{network, release::Candidate};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum CoreSource {
    #[default]
    Managed,
    External {
        path: PathBuf,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedRef {
    pub version: String,
    pub target: String,
    pub hash: Option<String>,
    pub source: Option<Candidate>,
}
impl ManagedRef {
    pub fn validate(&self) -> Result<(), String> {
        stable(&self.version)?;
        if self.target != crate::TARGET {
            return Err("受管核心平台不符".into());
        }
        if let Some(hash) = &self.hash {
            crate::updates::release::digest(hash)?;
        }
        if let Some(c) = &self.source {
            if c.component != crate::updates::Component::Engine
                || c.version != self.version
                || c.target != self.target
                || self.hash.is_none()
            {
                return Err("核心来源记录不符".into());
            }
            crate::updates::release::validate_engine(c)?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Trial {
    AwaitingStart,
    Starting,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FailureStage {
    Package,
    Compatibility,
    Startup,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocked {
    pub candidate: Candidate,
    pub stage: FailureStage,
    pub error: String,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedState {
    pub active: Option<ManagedRef>,
    pub previous: Option<ManagedRef>,
    pub trial: Option<Trial>,
    pub blocked: Option<Blocked>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdatePreferences {
    pub automatic: bool,
    pub automatic_core: bool,
    pub network: network::Preferences,
}
impl Default for UpdatePreferences {
    fn default() -> Self {
        Self {
            automatic: true,
            automatic_core: true,
            network: Default::default(),
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub core_revision: u64,
    pub preferences_revision: u64,
    pub core_source: CoreSource,
    pub managed: ManagedState,
    pub config_path: Option<PathBuf>,
    pub pending_config_path: Option<PathBuf>,
    pub update_preferences: UpdatePreferences,
    pub startup: Startup,
    pub desktop: Desktop,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Startup {
    pub restore_core_on_launch: bool,
    pub resume_core: bool,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Desktop {
    pub background_notice_shown: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 3,
            core_revision: 0,
            preferences_revision: 0,
            core_source: Default::default(),
            managed: Default::default(),
            config_path: None,
            pending_config_path: None,
            update_preferences: Default::default(),
            startup: Default::default(),
            desktop: Default::default(),
        }
    }
}
impl Settings {
    pub fn managed(&self) -> bool {
        self.core_source == CoreSource::Managed
    }
    pub fn selected_path(&self, root: &Path) -> Result<PathBuf, String> {
        match &self.core_source {
            CoreSource::External { path } => Ok(path.clone()),
            CoreSource::Managed => {
                let r = self
                    .managed
                    .active
                    .as_ref()
                    .ok_or("核心正在准备，请稍后重试")?;
                managed_path(root, r)
            }
        }
    }
}
pub fn managed_path(root: &Path, reference: &ManagedRef) -> Result<PathBuf, String> {
    reference.validate()?;
    Ok(root
        .join("engines/sing-box")
        .join(&reference.version)
        .join(&reference.target)
        .join(if cfg!(windows) {
            "sing-box.exe"
        } else {
            "sing-box"
        }))
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    let mut file = File::create(&temporary).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|e| e.to_string())?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| format!("保存 {} 失败：{e}", path.display()))
}
pub struct SettingsStore {
    pub root: PathBuf,
    state: Mutex<Settings>,
    writer: Mutex<()>,
    pub error: Option<String>,
    _lease: crate::instance::Lease,
}
impl SettingsStore {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        let lease = crate::instance::Lease::acquire(&root)?;
        Self::with_lease(root, lease)
    }
    pub fn with_lease(root: PathBuf, lease: crate::instance::Lease) -> Result<Self, String> {
        let path = root.join("settings.json");
        let read = (|| -> Result<Settings, String> {
            let bytes = match fs::read(&path) {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Settings::default())
                }
                Err(e) => return Err(e.to_string()),
            };
            let raw: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let schema = raw
                .get("schemaVersion")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let settings = match schema {
                0 => {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct Legacy {
                        core_path: Option<PathBuf>,
                        config_path: Option<PathBuf>,
                    }
                    let old: Legacy = serde_json::from_value(raw).map_err(|e| e.to_string())?;
                    Settings {
                        core_source: old
                            .core_path
                            .map(|path| CoreSource::External { path })
                            .unwrap_or_default(),
                        config_path: old.config_path,
                        ..Default::default()
                    }
                }
                1 => {
                    #[derive(Deserialize)]
                    #[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
                    enum Engine {
                        External { path: PathBuf },
                        Managed { version: String, target: String },
                    }
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct Preferences {
                        automatic: bool,
                        network: network::Preferences,
                    }
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct V1 {
                        schema_version: u32,
                        revision: u64,
                        engine: Option<Engine>,
                        config_path: Option<PathBuf>,
                        update_preferences: Preferences,
                    }
                    let old: V1 = serde_json::from_value(raw).map_err(|e| e.to_string())?;
                    let _ = (old.schema_version, old.revision);
                    let mut s = Settings {
                        config_path: old.config_path,
                        update_preferences: UpdatePreferences {
                            automatic: old.update_preferences.automatic,
                            automatic_core: old.update_preferences.automatic,
                            network: old.update_preferences.network,
                        },
                        ..Default::default()
                    };
                    match old.engine {
                        Some(Engine::External { path }) => {
                            s.core_source = CoreSource::External { path }
                        }
                        Some(Engine::Managed { version, target }) => {
                            s.managed.active = Some(ManagedRef {
                                version,
                                target,
                                hash: None,
                                source: None,
                            })
                        }
                        None => {}
                    }
                    s
                }
                2 => {
                    if raw.get("startup").is_some() || raw.get("desktop").is_some() {
                        return Err("旧设置包含不属于此版本的字段，已保留原文件".into());
                    }
                    let mut raw = raw;
                    raw["schemaVersion"] = serde_json::json!(3);
                    raw["startup"] = serde_json::to_value(Startup::default()).unwrap();
                    raw["desktop"] = serde_json::to_value(Desktop::default()).unwrap();
                    serde_json::from_value(raw).map_err(|e| e.to_string())?
                }
                3 => serde_json::from_value(raw).map_err(|e| e.to_string())?,
                _ => return Err("不支持此设置版本".into()),
            };
            Self::validate(&settings)?;
            if schema < 3 {
                let backup = root.join(format!("settings.v{schema}.backup.json"));
                if !backup.exists() {
                    let mut f = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(backup)
                        .map_err(|e| e.to_string())?;
                    f.write_all(&bytes)
                        .and_then(|_| f.sync_all())
                        .map_err(|e| e.to_string())?;
                }
                write_json(&path, &settings)?;
            }
            Ok(settings)
        })();
        let (settings, error) = match read {
            Ok(s) => (s, None),
            Err(e) => (
                Settings::default(),
                Some(format!("设置文件无法读取，已保留原文件：{e}")),
            ),
        };
        Ok(Self {
            root,
            state: Mutex::new(settings),
            writer: Mutex::new(()),
            error,
            _lease: lease,
        })
    }
    fn validate(s: &Settings) -> Result<(), String> {
        if s.schema_version != 3 {
            return Err("不支持此设置版本".into());
        }
        s.update_preferences.network.clone().validated()?;
        for r in [&s.managed.active, &s.managed.previous]
            .into_iter()
            .flatten()
        {
            r.validate()?;
        }
        if s.managed.trial.is_some() && (s.managed.active.is_none() || !s.managed()) {
            return Err("核心启动恢复记录无效".into());
        }
        if let Some(b) = &s.managed.blocked {
            crate::updates::release::validate_engine(&b.candidate)?;
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Settings {
        self.state.lock().unwrap().clone()
    }
    pub fn core(
        &self,
        expected: u64,
        change: impl FnOnce(&mut Settings) -> Result<(), String>,
    ) -> Result<Settings, String> {
        self.write(|s| {
            if s.core_revision != expected {
                return Err("核心选择或配置已改变，请重试".into());
            }
            change(s)?;
            s.core_revision += 1;
            Ok(())
        })
    }
    pub fn preferences(
        &self,
        expected: u64,
        preferences: UpdatePreferences,
    ) -> Result<Settings, String> {
        self.write(|s| {
            if s.preferences_revision != expected {
                return Err("偏好已改变，请载入最新设置后重试".into());
            }
            s.update_preferences = preferences;
            s.update_preferences.network = s.update_preferences.network.clone().validated()?;
            s.preferences_revision += 1;
            Ok(())
        })
    }
    pub fn restore_preference(&self, enabled: bool) -> Result<Settings, String> {
        self.write(|s| {
            s.startup.restore_core_on_launch = enabled;
            Ok(())
        })
    }
    pub fn resume(&self, enabled: bool) -> Result<Settings, String> {
        self.write(|s| {
            s.startup.resume_core = enabled;
            Ok(())
        })
    }
    pub fn confirm_start(&self, expected: u64) -> Result<Settings, String> {
        self.write(|s| {
            if s.core_revision != expected {
                return Err("核心选择已改变".into());
            }
            if s.managed.trial.is_some() {
                s.managed.trial = None;
                s.core_revision += 1;
            }
            s.startup.resume_core = true;
            Ok(())
        })
    }
    pub fn background_notice(&self) -> Result<Settings, String> {
        self.write(|s| {
            s.desktop.background_notice_shown = true;
            Ok(())
        })
    }
    fn write(
        &self,
        change: impl FnOnce(&mut Settings) -> Result<(), String>,
    ) -> Result<Settings, String> {
        let _writer = self.writer.lock().unwrap();
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let mut next = self.snapshot();
        change(&mut next)?;
        Self::validate(&next)?;
        write_json(&self.root.join("settings.json"), &next)?;
        *self.state.lock().unwrap() = next.clone();
        Ok(next)
    }
}
pub fn stable(value: &str) -> Result<semver::Version, String> {
    let v = semver::Version::parse(value).map_err(|e| e.to_string())?;
    if !v.pre.is_empty() || !v.build.is_empty() {
        return Err("只支持正式版本".into());
    }
    Ok(v)
}
