use crate::instance::{Endpoint, Lease, RequestError};
use std::{
    ffi::OsString,
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct Launch {
    pub login: bool,
    pub restart: bool,
    pub smoke: bool,
    pub data: Option<PathBuf>,
}
impl Launch {
    pub fn parse(args: &[OsString]) -> Result<Self, String> {
        let smoke = args.iter().any(|a| a == "--smoke-test");
        let data = args
            .iter()
            .position(|a| a == "--test-data-dir")
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from);
        if smoke && data.is_none() {
            return Err("隔离验收必须指定数据目录".into());
        }
        let restart = args.iter().any(|a| a == "--restart-from");
        if restart
            && args
                .iter()
                .position(|a| a == "--restart-from")
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.to_str())
                .and_then(|v| v.parse::<u32>().ok())
                .is_none()
        {
            return Err("更新重启来源无效".into());
        }
        Ok(Self {
            login: args.iter().any(|a| a == "--login-start") && !restart,
            restart,
            smoke,
            data,
        })
    }
    pub fn restart_environment(args: &[OsString]) -> tauri::Env {
        let mut env = tauri::Env::default();
        let mut next = Vec::new();
        let mut skip = false;
        for arg in args {
            if skip {
                skip = false;
                continue;
            }
            if arg == "--restart-from" {
                skip = true;
                continue;
            }
            if arg == "--login-start" {
                continue;
            }
            next.push(arg.clone());
        }
        next.extend([
            OsString::from("--restart-from"),
            std::process::id().to_string().into(),
        ]);
        env.args_os = next;
        env
    }
    pub fn admit(&self, root: &std::path::Path) -> Result<Option<Lease>, String> {
        let endpoint = Endpoint::new(root)?;
        let deadline = Instant::now() + Duration::from_secs(if self.restart { 15 } else { 4 });
        loop {
            match Lease::acquire(root) {
                Ok(lease) => return Ok(Some(lease)),
                Err(error) => {
                    if self.login {
                        return Ok(None);
                    }
                    if !self.restart {
                        match endpoint.activate() {
                            Ok(()) => return Ok(None),
                            Err(RequestError::Closing) => {
                                return Err("Sing Shell 正在退出或重启，请稍后打开".into())
                            }
                            Err(RequestError::Unavailable) => {}
                        }
                    }
                    if Instant::now() >= deadline {
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_consumes_login_and_previous_marker() {
        let args: Vec<OsString> = ["app", "--login-start", "--restart-from", "123"]
            .into_iter()
            .map(Into::into)
            .collect();
        let env = Launch::restart_environment(&args);
        assert!(!env.args_os.iter().any(|a| a == "--login-start"));
        assert_eq!(
            env.args_os
                .iter()
                .filter(|a| *a == "--restart-from")
                .count(),
            1
        );
        assert!(Launch::parse(&env.args_os).unwrap().restart);
    }
}
