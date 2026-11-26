use super::Connection;
use crate::{
    logs::Log,
    platform::{authorization, network},
    runtime::RuntimeSnapshot,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Request {
    Start {
        input: Connection,
        core_version: String,
    },
    Poll,
    Stop,
    Quit,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub runtime: RuntimeSnapshot,
    pub network: network::Observation,
    pub phase: String,
    pub error: Option<String>,
    pub logs: Vec<Log>,
    pub failure: Option<Failure>,
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
            phase: "stopped".into(),
            error: None,
            logs: vec![],
            failure: None,
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
    tokio::time::timeout(Duration::from_secs(20), async {
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
