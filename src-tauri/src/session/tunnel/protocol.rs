use super::ConnectionInput;
use crate::{
    logs::Log,
    platform::{authorization, network},
    runtime::RuntimeSnapshot,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub(super) const STOP_BUDGET: Duration = Duration::from_secs(55);
pub(super) const DISCONNECT_BUDGET: Duration = Duration::from_secs(65);
#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Request {
    Start {
        input: ConnectionInput,
        core_version: String,
        operation_id: u64,
    },
    CoreBegin {
        version: String,
        material: Option<crate::core::material::Envelope>,
        operation_id: u64,
    },
    CoreChunk {
        offset: u64,
        bytes: Vec<u8>,
    },
    CoreFinish,
    Poll,
    Activity {
        query: crate::activity::Query,
    },
    Logs {
        after: u64,
        limit: usize,
    },
    Stop,
    Quit,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub runtime: RuntimeSnapshot,
    pub network: network::Observation,
    pub phase: Phase,
    pub error: Option<String>,
    pub logs: Vec<Log>,
    pub failure: Option<Failure>,
    pub authorization_stage: Option<authorization::Stage>,
    pub session_id: Option<String>,
    pub running_node: Option<crate::profile::NodeSummary>,
    pub running_connection_options: Option<crate::profile::ConnectionOptions>,
    pub core_received: Option<u64>,
    pub operation_id: u64,
    pub operation: Option<String>,
    pub operation_error: Option<String>,
    pub activity_summary: Option<crate::activity::Summary>,
    pub activity_page: Option<crate::activity::Page>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Stopped,
    Authorizing,
    Preparing,
    Starting,
    Running,
    Stopping,
    Failed,
    CleanupRequired,
}
impl Phase {
    pub fn runtime_status(self) -> &'static str {
        match self {
            Self::Authorizing | Self::Preparing | Self::Starting => "starting",
            Self::CleanupRequired => "failed",
            phase => phase.as_str(),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Authorizing => "authorizing",
            Self::Preparing => "preparing",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
            Self::CleanupRequired => "cleanupRequired",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Failure {
    Preparation,
    CoreStart,
    Network,
    Cleanup,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            runtime: Default::default(),
            network: Default::default(),
            phase: Phase::Stopped,
            error: None,
            logs: vec![],
            failure: None,
            authorization_stage: None,
            session_id: None,
            running_node: None,
            running_connection_options: None,
            core_received: None,
            operation_id: 0,
            operation: None,
            operation_error: None,
            activity_summary: None,
            activity_page: None,
        }
    }
}
pub(super) async fn write(
    channel: &mut authorization::Channel,
    value: &impl Serialize,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "网络控制消息无法编码")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("网络控制消息超限".into());
    }
    channel
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|e| e.to_string())?;
    channel.write_all(&bytes).await.map_err(|e| e.to_string())?;
    channel.flush().await.map_err(|e| e.to_string())
}
pub(super) async fn read<T: serde::de::DeserializeOwned>(
    channel: &mut authorization::Channel,
) -> Result<T, String> {
    let count = channel.read_u32().await.map_err(|e| e.to_string())? as usize;
    if count > 2 * 1024 * 1024 {
        return Err("网络控制消息超限".into());
    }
    let mut bytes = vec![0; count];
    channel
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "网络控制消息格式无效".into())
}
pub(super) async fn exchange(
    channel: &mut authorization::Channel,
    request: &Request,
) -> Result<Snapshot, String> {
    let budget = if matches!(request, Request::Stop | Request::Quit) {
        STOP_BUDGET
    } else {
        Duration::from_secs(20)
    };
    tokio::time::timeout(budget, async {
        write(channel, request).await?;
        read(channel).await
    })
    .await
    .map_err(|_| "授权进程未回应，请重试停止")?
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Closed {
    pub closed: bool,
}
