use super::*;
use std::os::unix::fs::MetadataExt;
pub fn elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}
fn verify(
    stream: &tokio::net::UnixStream,
    expected_pid: Option<u32>,
    root: bool,
) -> Result<(), String> {
    let credential = stream.peer_cred().map_err(|e| e.to_string())?;
    if root && credential.uid() != 0 {
        return Err("网络进程不属于系统管理员".into());
    }
    #[cfg(target_os = "linux")]
    let pid = credential
        .pid()
        .map(|p| p as u32)
        .ok_or("无法核对网络控制端进程")?;
    #[cfg(target_os = "macos")]
    let pid = {
        use std::os::fd::AsRawFd;
        let mut pid: libc::pid_t = 0;
        let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        pid as u32
    };
    if expected_pid.is_some_and(|expected| expected != pid) {
        return Err("网络控制端进程已改变".into());
    }
    #[cfg(target_os = "linux")]
    let path = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    return super::macos::verify(stream, pid, root);
    #[cfg(target_os = "linux")]
    same_executable(&path)
}
pub async fn direct_connect(
    root: &Path,
    cancel: &AtomicBool,
    progress: &impl Fn(Stage),
) -> Result<Connection, String> {
    progress(Stage::Registration);
    let _ = root;
    use std::os::unix::fs::DirBuilderExt;
    let directory = PathBuf::from("/tmp").join(format!(
        "ssn-{}-{}",
        unsafe { libc::geteuid() },
        &nonce()?[..16]
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|e| e.to_string())?;
    let endpoint = directory.join("control.sock");
    let listener = tokio::net::UnixListener::bind(&endpoint).map_err(|e| e.to_string())?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            if let Some(parent) = self.0.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
    let _cleanup = Cleanup(endpoint.clone());
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = if elevated() {
        std::process::Command::new(&executable)
    } else {
        #[cfg(target_os = "linux")]
        {
            let mut command = std::process::Command::new("/usr/bin/pkexec");
            command.arg(&executable);
            command
        }
        #[cfg(target_os = "macos")]
        {
            super::macos::launcher(&executable, &endpoint)
        }
    };
    #[cfg(target_os = "macos")]
    let apple_script = !elevated();
    #[cfg(not(target_os = "macos"))]
    let apple_script = false;
    if !apple_script {
        command
            .arg("--network-worker")
            .arg(&endpoint)
            .arg(std::process::id().to_string());
    }
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("无法请求系统网络授权：{e}"))?;
    #[cfg(target_os = "macos")]
    let mut process = Process::Mac {
        token: None,
        launcher: child,
    };
    #[cfg(not(target_os = "macos"))]
    let mut process = Process::Child(child);
    let accepted = async {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    let stream = loop {
        tokio::select! {result=listener.accept()=>{break result.map_err(|e|e.to_string())?.0},_=tokio::time::sleep(Duration::from_millis(100))=>{if cancel.load(Ordering::Acquire){return Err("网络授权已取消".into());}if process.exited()?{return Err("系统未允许网络授权，请重试".into());}if tokio::time::Instant::now()>=deadline{return Err("网络授权超时，请重试".into());}}}
    };
    Ok::<_, String>(stream)
    }.await;
    let mut stream = match accepted {
        Ok(stream) => stream,
        Err(error) => {
            drop(listener);
            process.finish().await;
            return Err(error);
        }
    };

    progress(Stage::Activation);
    let result = handshake::bounded(
        cancel,
        handshake::ACTIVATION,
        handshake::client(&mut stream, |s| {
            progress(Stage::Authentication);
            #[cfg(target_os = "macos")]
            let peer_token = super::macos::token(s)?;
            verify(s, None, true)?;
            #[cfg(target_os = "macos")]
            if let Process::Mac { token, .. } = &mut process {
                *token = Some(peer_token);
            }
            progress(Stage::Protocol);
            Ok(())
        }),
    )
    .await;
    let connection = Connection {
        channel: Box::new(stream),
        process: Some(process),
    };
    if let Err(error) = result {
        connection.finish().await?;
        return Err(error);
    }
    Ok(connection)
}
#[cfg(target_os = "linux")]
pub async fn connect(
    root: &Path,
    cancel: &AtomicBool,
    progress: &impl Fn(Stage),
) -> Result<Connection, String> {
    direct_connect(root, cancel, progress).await
}
#[cfg(target_os = "macos")]
pub async fn connect(
    root: &Path,
    cancel: &AtomicBool,
    progress: &impl Fn(Stage),
) -> Result<Connection, String> {
    if !elevated() {
        progress(Stage::Registration);
        super::macos::prepare()?;
    } else if std::env::var("SING_SHELL_NETWORK_PREFLIGHT").as_deref() != Ok("1") {
        return Err("请以普通用户运行 Sing Shell，TUN 将单独请求系统授权".into());
    }
    direct_connect(root, cancel, progress).await
}
pub async fn worker(endpoint: &Path, parent: u32) -> Result<WorkerPeer, String> {
    if !elevated() {
        return Err("网络进程必须由系统授权启动".into());
    }
    let owner = std::fs::metadata(endpoint.parent().ok_or("网络端点无父目录")?)
        .map_err(|e| e.to_string())?;
    if owner.mode() & 0o077 != 0 {
        return Err("网络端点目录权限无效".into());
    }
    let mut stream = tokio::net::UnixStream::connect(endpoint)
        .await
        .map_err(|e| e.to_string())?;
    verify(&stream, Some(parent), false)?;
    let uid = stream.peer_cred().map_err(|e| e.to_string())?.uid();
    handshake::bounded(
        &AtomicBool::new(false),
        Duration::from_secs(3),
        handshake::server(&mut stream),
    )
    .await?;
    Ok(WorkerPeer {
        channel: Box::new(stream),
        owner_key: crate::rulesets::digest(format!("uid:{uid}").as_bytes()),
    })
}
pub fn protected_root() -> Result<PathBuf, String> {
    Ok(PathBuf::from(if cfg!(target_os = "macos") {
        "/Library/Application Support/Sing Shell Network"
    } else {
        "/var/lib/sing-shell-network"
    }))
}
