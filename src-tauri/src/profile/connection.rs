use super::{routing::Routing, Mode, Node, Profile};
use crate::rulesets::Version;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionOptions {
    pub ipv4_compatibility: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionInput {
    pub node: Node,
    pub routing: Routing,
    pub versions: Vec<Version>,
    pub core_version: String,
    #[serde(default)]
    pub options: ConnectionOptions,
}

impl ConnectionInput {
    pub fn from_profile(
        profile: &Profile,
        node: Node,
        versions: Vec<Version>,
        core_version: String,
    ) -> Result<Self, String> {
        if profile.mode != Mode::Generated {
            return Err("当前为原始配置模式".into());
        }
        Ok(Self {
            node,
            routing: profile.routing.clone(),
            versions,
            core_version,
            options: profile.connection,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.node.id.is_empty() || self.node.name.is_empty() || self.node.name.len() > 256 {
            return Err("节点标识或名称无效".into());
        }
        super::normalize_tags(self.node.tags.clone())?;
        self.node.spec.validate()?;
        self.routing.validate()?;
        super::tun::require_version(&self.core_version)?;
        let expected: Vec<_> = self
            .routing
            .policies
            .iter()
            .filter(|p| p.enabled)
            .filter_map(|p| match &p.r#match {
                super::routing::Match::Source { source_id } => Some(source_id),
                _ => None,
            })
            .collect();
        if expected.len() != self.versions.len() {
            return Err("固定来源数量不符".into());
        }
        let mut unique = std::collections::BTreeSet::new();
        for version in &self.versions {
            version.validate()?;
            if !expected.contains(&&version.source_id) || !unique.insert(&version.source_id) {
                return Err("固定来源不符或重复".into());
            }
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> String {
        crate::rulesets::digest(
            &serde_json::to_vec(&(
                &self.node.spec,
                self.routing.effective(),
                &self.versions,
                self.options,
            ))
            .expect("connection serialization"),
        )
    }
}
