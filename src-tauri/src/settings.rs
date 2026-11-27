use crate::updates::{network, release::Candidate};
use serde::{Deserialize, Serialize};
use std::{
    fs,
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
    pub nodes_revision: u64,
    pub routing_revision: u64,
    #[serde(default)]
    pub connection_revision: u64,
    pub rule_preferences: Vec<crate::rulesets::service::Preference>,
    pub profile: crate::profile::Profile,
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
    #[serde(default)]
    pub last_connection: Option<crate::profile::ConnectionInput>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Desktop {
    pub background_notice_shown: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 5,
            core_revision: 0,
            preferences_revision: 0,
            nodes_revision: 0,
            routing_revision: 0,
            connection_revision: 0,
            rule_preferences: crate::rulesets::service::defaults(),
            profile: Default::default(),
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
    pub fn public(&self) -> serde_json::Value {
        serde_json::json!({"schemaVersion":self.schema_version,"coreRevision":self.core_revision,"nodesRevision":self.nodes_revision,"routingRevision":self.routing_revision,"rulePreferences":self.rule_preferences,"preferencesRevision":self.preferences_revision,"connectionRevision":self.connection_revision,"profile":self.profile.public(),"coreSource":self.core_source,"managed":self.managed,"configPath":self.config_path,"pendingConfigPath":self.pending_config_path,"updatePreferences":self.update_preferences,"startup":{"restoreCoreOnLaunch":self.startup.restore_core_on_launch,"resumeCore":self.startup.resume_core},"desktop":self.desktop})
    }
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
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    crate::private_files::write(path, &bytes)
}
pub struct SettingsStore {
    pub root: PathBuf,
    state: Mutex<Settings>,
    writer: Mutex<()>,
    error: Mutex<Option<String>>,
    _lease: crate::instance::Lease,
}
impl SettingsStore {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        let lease = crate::instance::Lease::acquire(&root)?;
        Self::with_lease(root, lease)
    }
    pub fn with_lease(root: PathBuf, lease: crate::instance::Lease) -> Result<Self, String> {
        crate::private_files::directory(&root)?;
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
            if schema != 5 {
                return Err("大版本数据格式已变更，请备份并重新初始化".into());
            }
            let settings: Settings = serde_json::from_value(raw).map_err(|e| e.to_string())?;
            Self::validate(&settings)?;
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
            error: Mutex::new(error),
            _lease: lease,
        })
    }
    fn validate(s: &Settings) -> Result<(), String> {
        if s.schema_version != 5 {
            return Err("不支持此设置版本".into());
        }
        s.update_preferences.network.clone().validated()?;
        s.profile.validate()?;
        for p in &s.rule_preferences {
            p.validate()?;
        }
        if let Some(last) = &s.startup.last_connection {
            last.validate()?;
        }
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
    pub fn profile(
        &self,
        expected: u64,
        change: impl FnOnce(&mut crate::profile::Profile) -> Result<(), String>,
    ) -> Result<Settings, String> {
        self.write(|s| {
            if s.nodes_revision != expected {
                return Err("节点列表已改变，请载入最新设置后重试".into());
            }
            change(&mut s.profile)?;
            if s.startup.last_connection.as_ref().is_some_and(|last| {
                s.profile
                    .nodes
                    .iter()
                    .all(|node| node.id != last.node.id || node.spec != last.node.spec)
            }) {
                s.startup.last_connection = None;
                s.startup.resume_core = false;
            }
            s.nodes_revision += 1;
            Ok(())
        })
    }
    pub fn select_node(&self, id: &str, expected: u64) -> Result<Settings, String> {
        self.write(|s| {
            if s.nodes_revision != expected {
                return Err("节点列表已改变，请重新打开菜单或载入最新设置后重试".into());
            }
            if s.profile.mode != crate::profile::Mode::Generated {
                return Err("请先停止并切换到节点模式".into());
            }
            if !s.profile.nodes.iter().any(|n| n.id == id) {
                return Err("节点不存在".into());
            }
            if s.profile.selected_node.as_deref() != Some(id) {
                s.profile.selected_node = Some(id.into());
                s.nodes_revision += 1;
            }
            Ok(())
        })
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    pub fn backup_reset(&self) -> Result<PathBuf, String> {
        let _writer = self.writer.lock().unwrap();
        if self.error().is_none() {
            return Err("设置正常，无需重置".into());
        }
        let original = self.root.join("settings.json");
        let backup = self
            .root
            .join(format!("settings.{}.backup.json", crate::logs::now()));
        let bytes = fs::read(&original).map_err(|e| e.to_string())?;
        crate::private_files::write(&backup, &bytes)?;
        let settings = Settings::default();
        write_json(&original, &settings)?;
        *self.state.lock().unwrap() = settings;
        *self.error.lock().unwrap() = None;
        Ok(backup)
    }
    pub fn routing(
        &self,
        expected: u64,
        routing: crate::profile::routing::Routing,
    ) -> Result<Settings, String> {
        let routing = routing.normalized()?;
        self.write(|s| {
            if s.routing_revision != expected {
                return Err("分流已改变，草稿已保留，请载入最新规则核对".into());
            }
            let ids = routing.source_ids();
            s.rule_preferences.retain(|p| ids.contains(&p.source_id));
            for id in ids {
                if !s.rule_preferences.iter().any(|p| p.source_id == id) {
                    s.rule_preferences
                        .push(crate::rulesets::service::Preference::new(id));
                }
            }
            s.profile.routing = routing;
            s.routing_revision += 1;
            Ok(())
        })
    }
    pub fn rule_preference(
        &self,
        preference: crate::rulesets::service::Preference,
    ) -> Result<Settings, String> {
        preference.validate()?;
        self.write(|s| {
            if !s
                .profile
                .routing
                .source_ids()
                .contains(&preference.source_id)
            {
                return Err("请先保存包含此来源的规则表".into());
            }
            let slot = s
                .rule_preferences
                .iter_mut()
                .find(|p| p.source_id == preference.source_id)
                .ok_or("来源偏好不存在")?;
            *slot = preference;
            Ok(())
        })
    }
    pub fn restore_preference(&self, enabled: bool) -> Result<Settings, String> {
        self.write(|s| {
            s.startup.restore_core_on_launch = enabled;
            Ok(())
        })
    }
    pub fn connection_options(
        &self,
        expected: u64,
        options: crate::profile::ConnectionOptions,
    ) -> Result<Settings, String> {
        self.write(|s| {
            if s.connection_revision != expected {
                return Err("连接设置已改变，草稿已保留，请载入最新设置后重试".into());
            }
            if s.profile.mode != crate::profile::Mode::Generated {
                return Err("原始配置的连接行为由配置文件控制".into());
            }
            s.profile.connection = options;
            s.connection_revision += 1;
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
        self.confirm_connection(expected, None)
    }
    pub fn confirm_connection(
        &self,
        expected: u64,
        input: Option<crate::profile::ConnectionInput>,
    ) -> Result<Settings, String> {
        self.write(|s| {
            if s.core_revision != expected {
                return Err("核心选择已改变".into());
            }
            if s.managed.trial.is_some() {
                s.managed.trial = None;
                s.core_revision += 1;
            }
            s.startup.resume_core = true;
            s.startup.last_connection = input;
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
        if let Some(e) = self.error() {
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
