use crate::updates::network;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum Engine {
    External { path: PathBuf },
    Managed { version: String, target: String },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdatePreferences {
    pub automatic: bool,
    pub network: network::Preferences,
}
impl Default for UpdatePreferences {
    fn default() -> Self {
        Self {
            automatic: true,
            network: Default::default(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub revision: u64,
    pub engine: Option<Engine>,
    pub config_path: Option<PathBuf>,
    pub update_preferences: UpdatePreferences,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            engine: None,
            config_path: None,
            update_preferences: Default::default(),
        }
    }
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
    _lock: File,
}
impl SettingsStore {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("instance.lock"))
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while let Err(e) = lock.try_lock() {
            if Instant::now() >= deadline {
                return Err(format!("数据目录正在使用，或无法锁定：{e}"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
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
            if raw.get("schemaVersion").is_none() {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Legacy {
                    core_path: Option<PathBuf>,
                    config_path: Option<PathBuf>,
                }
                let legacy: Legacy = serde_json::from_value(raw).map_err(|e| e.to_string())?;
                let settings = Settings {
                    engine: legacy.core_path.map(|path| Engine::External { path }),
                    config_path: legacy.config_path,
                    ..Default::default()
                };
                let backup = root.join("settings.v0.backup.json");
                if !backup.exists() {
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(backup)
                        .map_err(|e| e.to_string())?;
                    file.write_all(&bytes)
                        .and_then(|_| file.sync_all())
                        .map_err(|e| e.to_string())?;
                }
                write_json(&path, &settings)?;
                return Ok(settings);
            }
            let settings: Settings = serde_json::from_value(raw).map_err(|e| e.to_string())?;
            if settings.schema_version != 1 {
                return Err("不支持此设置版本".into());
            }
            settings.update_preferences.network.clone().validated()?;
            if let Some(Engine::Managed { version, target }) = &settings.engine {
                stable(version)?;
                if target != crate::TARGET {
                    return Err("受管核心的平台与本机不符".into());
                }
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
            _lock: lock,
        })
    }
    pub fn snapshot(&self) -> Settings {
        self.state.lock().unwrap().clone()
    }
    pub fn save(
        &self,
        expected: u64,
        change: impl FnOnce(&mut Settings),
    ) -> Result<Settings, String> {
        let _write = self.writer.lock().unwrap();
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let mut next = self.snapshot();
        if next.revision != expected {
            return Err("设置已改变，请载入最新设置后重试".into());
        }
        change(&mut next);
        next.revision += 1;
        next.update_preferences.network = next.update_preferences.network.validated()?;
        write_json(&self.root.join("settings.json"), &next)?;
        *self.state.lock().unwrap() = next.clone();
        Ok(next)
    }
    pub fn engine_path(&self, engine: &Engine) -> Result<PathBuf, String> {
        match engine {
            Engine::External { path } => Ok(path.clone()),
            Engine::Managed { version, target } => {
                stable(version)?;
                if target != crate::TARGET {
                    return Err("受管核心的平台无效".into());
                }
                Ok(self
                    .root
                    .join("engines/sing-box")
                    .join(version)
                    .join(target)
                    .join(if cfg!(windows) {
                        "sing-box.exe"
                    } else {
                        "sing-box"
                    }))
            }
        }
    }
}
pub fn stable(value: &str) -> Result<semver::Version, String> {
    let version = semver::Version::parse(value).map_err(|e| e.to_string())?;
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err("只支持正式版本".into());
    }
    Ok(version)
}
