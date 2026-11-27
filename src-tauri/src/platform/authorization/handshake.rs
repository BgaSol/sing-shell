use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const VERSION: u32 = 4;
const LIMIT: usize = 1024;
pub const ACTIVATION: Duration = Duration::from_secs(15);

#[derive(Serialize, Deserialize)]
#[serde(tag = "handshake", rename_all = "camelCase", deny_unknown_fields)]
enum Frame {
    Ready { version: u32, nonce: String },
    Confirm { version: u32, nonce: String },
    Accepted { nonce: String },
}
async fn read<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Frame, String> {
    let size = stream
        .read_u32()
        .await
        .map_err(|e| format!("网络授权握手读取失败：{e}"))? as usize;
    if size == 0 || size > LIMIT {
        return Err("网络授权握手消息超限".into());
    }
    let mut bytes = vec![0; size];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes)
        .map_err(|_| "网络授权握手格式或协议不兼容，请退出旧实例后重试".into())
}
async fn write<S: AsyncWrite + Unpin>(stream: &mut S, frame: &Frame) -> Result<(), String> {
    let bytes = serde_json::to_vec(frame).map_err(|e| e.to_string())?;
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(&bytes).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())
}
pub async fn bounded<T>(
    cancel: &AtomicBool,
    duration: Duration,
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let cancellation = async {
        loop {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::select! {
        biased;
        _ = cancellation => Err("网络授权已取消".into()),
        result = tokio::time::timeout(duration, work) => result.map_err(|_| "网络授权握手超时".to_string())?,
    }
}
pub async fn client<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    verify: impl FnOnce(&S) -> Result<(), String>,
) -> Result<(), String> {
    let nonce = match read(stream).await? {
        Frame::Ready {
            version: VERSION,
            nonce,
        } if nonce.len() == 48 && nonce.bytes().all(|b| b.is_ascii_hexdigit()) => nonce,
        _ => return Err("网络 helper 协议不兼容，请退出旧实例并重新打开当前应用".into()),
    };
    // A launchd socket reports its creator until the helper accepts it.
    verify(stream)?;
    write(
        stream,
        &Frame::Confirm {
            version: VERSION,
            nonce: nonce.clone(),
        },
    )
    .await?;
    match read(stream).await? {
        Frame::Accepted { nonce: accepted } if accepted == nonce => Ok(()),
        _ => Err("网络 helper 未确认当前授权会话".into()),
    }
}
pub async fn server<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) -> Result<(), String> {
    let nonce = super::nonce()?;
    write(
        stream,
        &Frame::Ready {
            version: VERSION,
            nonce: nonce.clone(),
        },
    )
    .await?;
    match read(stream).await? {
        Frame::Confirm {
            version: VERSION,
            nonce: confirmation,
        } if confirmation == nonce => {}
        _ => return Err("网络控制协议或会话标识不符".into()),
    }
    write(stream, &Frame::Accepted { nonce }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(work: impl std::future::Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(work);
    }
    #[test]
    fn delayed_ready_is_authenticated_before_confirmation() {
        run(async {
            let (mut a, mut b) = tokio::io::duplex(2048);
            let authenticated = AtomicBool::new(false);
            let (client, server) = tokio::join!(
                client(&mut a, |_| {
                    authenticated.store(true, Ordering::Release);
                    Ok(())
                }),
                async {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    server(&mut b).await
                }
            );
            client.unwrap();
            server.unwrap();
            assert!(authenticated.load(Ordering::Acquire));
        });
    }
    #[test]
    fn untrusted_peer_receives_no_confirmation() {
        run(async {
            let (mut a, mut b) = tokio::io::duplex(2048);
            write(
                &mut b,
                &Frame::Ready {
                    version: VERSION,
                    nonce: "a".repeat(48),
                },
            )
            .await
            .unwrap();
            assert_eq!(
                client(&mut a, |_| Err("untrusted".into()))
                    .await
                    .unwrap_err(),
                "untrusted"
            );
            assert!(tokio::time::timeout(Duration::from_millis(20), b.read_u8())
                .await
                .is_err());
        });
    }
    #[test]
    fn cancellation_timeout_and_oversized_frames_fail_closed() {
        run(async {
            let (mut a, mut b) = tokio::io::duplex(2048);
            let cancel = AtomicBool::new(true);
            assert!(bounded(&cancel, ACTIVATION, client(&mut a, |_| Ok(())))
                .await
                .unwrap_err()
                .contains("取消"));
            cancel.store(false, Ordering::Release);
            assert!(bounded(
                &cancel,
                Duration::from_millis(20),
                client(&mut a, |_| Ok(()))
            )
            .await
            .unwrap_err()
            .contains("超时"));
            b.write_u32(1025).await.unwrap();
            assert!(client(&mut a, |_| panic!("must not verify invalid frame"))
                .await
                .unwrap_err()
                .contains("超限"));
        });
    }
    #[test]
    fn wrong_nonce_and_old_protocol_are_rejected() {
        run(async {
            let (mut a, mut b) = tokio::io::duplex(2048);
            let (server, _) = tokio::join!(server(&mut a), async {
                let _ = read(&mut b).await.unwrap();
                write(
                    &mut b,
                    &Frame::Confirm {
                        version: VERSION,
                        nonce: "b".repeat(48),
                    },
                )
                .await
                .unwrap();
            });
            assert!(server.is_err());
            let (mut a, mut b) = tokio::io::duplex(2048);
            write(
                &mut b,
                &Frame::Ready {
                    version: 1,
                    nonce: "a".repeat(48),
                },
            )
            .await
            .unwrap();
            assert!(client(&mut a, |_| panic!("old protocol")).await.is_err());
        });
    }
}
