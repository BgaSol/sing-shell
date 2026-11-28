use super::routing::{Action, Match, Policy};
use crate::{rulesets::service::Preference, settings::Settings};
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Removed {
    pub policy: Policy,
    pub index: usize,
    pub preference: Option<Preference>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Change {
    Add { policy: Policy },
    Edit { policy: Policy },
    AddSources { ids: Vec<String> },
    Exit { id: String, exit: Action },
    Enabled { id: String, enabled: bool },
    Move { id: String, direction: i8 },
    Default { exit: Action },
    Remove { id: String },
    Restore { removed: Removed },
}

impl Change {
    pub(crate) fn apply(self, s: &mut Settings) -> Result<Option<Removed>, super::routing::Issue> {
        if s.profile.mode != super::Mode::Generated {
            return Err("原始配置模式下不能修改分流".into());
        }
        let policies = &mut s.profile.routing.policies;
        let mut removed = None;
        let adding = matches!(&self, Self::Add { .. });
        match self {
            Self::Add { mut policy } | Self::Edit { mut policy } => {
                if !matches!(policy.r#match, Match::Custom { .. }) {
                    return Err("此入口只编辑自定义规则".into());
                }
                if policy.exit == Action::Proxy {
                    policy.bypass_tun = false;
                }
                if policy.name.trim().is_empty() {
                    if let Match::Custom { domains, ip_cidrs } = &policy.r#match {
                        policy.name = domains
                            .first()
                            .map(|d| d.value.trim().to_owned())
                            .or_else(|| ip_cidrs.first().cloned())
                            .unwrap_or_default()
                            .chars()
                            .take(64)
                            .collect();
                    }
                }
                let position = policies.iter().position(|p| p.id == policy.id);
                if adding {
                    if position.is_some() {
                        return Err("规则已经存在，请重新打开编辑器".into());
                    }
                    policies.push(policy);
                } else {
                    let index = position.ok_or("规则已移除，请重新打开列表")?;
                    if !matches!(policies[index].r#match, Match::Custom { .. }) {
                        return Err("不能替换站点来源".into());
                    }
                    policies[index] = policy;
                }
            }
            Self::AddSources { ids } => {
                if ids.is_empty() || ids.len() > 5 {
                    return Err("请选择有效的站点来源".into());
                }
                for id in ids {
                    let definition = crate::rulesets::definition(&id)?;
                    if policies.iter().any(
                        |p| matches!(&p.r#match, Match::Source { source_id } if source_id == &id),
                    ) {
                        return Err("同一站点来源只能添加一次".into());
                    }
                    policies.push(Policy {
                        id: format!("source-{id}"),
                        name: definition.name.into(),
                        enabled: true,
                        exit: if ["cn-domain", "cn-ip"].contains(&id.as_str()) {
                            Action::Direct
                        } else {
                            Action::Proxy
                        },
                        bypass_tun: false,
                        r#match: Match::Source { source_id: id },
                    });
                }
            }
            Self::Exit { id, exit } => {
                let policy = policies
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or("规则已移除")?;
                policy.exit = exit;
                if exit == Action::Proxy {
                    policy.bypass_tun = false;
                }
            }
            Self::Enabled { id, enabled } => {
                policies
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or("规则已移除")?
                    .enabled = enabled
            }
            Self::Move { id, direction } => {
                if ![-1, 1].contains(&direction) {
                    return Err("无效的移动方向".into());
                }
                let index = policies
                    .iter()
                    .position(|p| p.id == id)
                    .ok_or("规则已移除")?;
                let target = index as isize + direction as isize;
                if target < 0 || target >= policies.len() as isize {
                    return Err("规则已位于边界".into());
                }
                policies.swap(index, target as usize);
            }
            Self::Default { exit } => s.profile.routing.default_exit = exit,
            Self::Remove { id } => {
                let index = policies
                    .iter()
                    .position(|p| p.id == id)
                    .ok_or("规则已移除")?;
                let policy = policies.remove(index);
                let preference = match &policy.r#match {
                    Match::Source { source_id } => s
                        .rule_preferences
                        .iter()
                        .find(|p| &p.source_id == source_id)
                        .cloned(),
                    _ => None,
                };
                removed = Some(Removed {
                    policy,
                    index,
                    preference,
                });
            }
            Self::Restore { removed } => {
                if policies.iter().any(|p| {
                    p.id == removed.policy.id
                        || matches!((&p.r#match, &removed.policy.r#match),
                    (Match::Source { source_id: a }, Match::Source { source_id: b }) if a == b)
                }) {
                    return Err("无法撤销：规则或站点来源已重新添加".into());
                }
                if let Some(pref) = removed.preference {
                    pref.validate()?;
                    if !matches!(&removed.policy.r#match, Match::Source { source_id } if source_id == &pref.source_id)
                    {
                        return Err("恢复的来源偏好不符".into());
                    }
                    s.rule_preferences.retain(|p| p.source_id != pref.source_id);
                    s.rule_preferences.push(pref);
                }
                policies.insert(removed.index.min(policies.len()), removed.policy);
            }
        }
        s.profile.routing = s.profile.routing.checked()?;
        let ids = s.profile.routing.source_ids();
        s.rule_preferences.retain(|p| ids.contains(&p.source_id));
        for id in ids {
            if !s.rule_preferences.iter().any(|p| p.source_id == id) {
                s.rule_preferences.push(Preference::new(id));
            }
        }
        Ok(removed)
    }
}
