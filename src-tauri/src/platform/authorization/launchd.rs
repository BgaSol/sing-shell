const SERVICE: &str = "system/com.bgasol.singshell.network";

#[derive(Debug, Default)]
pub(super) struct Job {
    manager: String,
    bundle: String,
    program: String,
    state: String,
    active: Option<u32>,
    pid: Option<u32>,
    version: String,
    job_state: String,
    exit: String,
}
impl Job {
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()?.trim() != format!("{SERVICE} = {{") {
            return None;
        }
        let mut job = Self::default();
        let mut depth = 1usize;
        let mut seen = std::collections::HashSet::new();
        for line in lines {
            let line = line.trim();
            if depth == 1 {
                if let Some((key, value)) = line.split_once(" = ") {
                    if !seen.insert(key) || value.len() > 256 {
                        return None;
                    }
                    match key {
                        "managed_by" => job.manager = value.into(),
                        "parent bundle identifier" => job.bundle = value.into(),
                        "program identifier" | "program" => job.program = value.into(),
                        "state" => job.state = value.into(),
                        "active count" => job.active = Some(value.parse().ok()?),
                        "pid" => job.pid = Some(value.parse().ok()?),
                        "parent bundle version" => job.version = value.into(),
                        "job state" => job.job_state = value.into(),
                        "last exit code" => job.exit = value.into(),
                        _ => {}
                    }
                }
            }
            if line.ends_with('{') {
                depth += 1;
            } else if line == "}" {
                depth = depth.checked_sub(1)?;
            }
        }
        (depth == 0).then_some(job)
    }
    pub fn failed_inactive_registration(&self) -> bool {
        self.manager == "com.apple.xpc.ServiceManagement"
            && self.bundle == "com.bgasol.singshell"
            && matches!(
                self.program.as_str(),
                "Contents/MacOS/sing-shell (mode: 2)" | "Contents/MacOS/sing-shell"
            )
            && self.active == Some(0)
            && self.pid.is_none()
            && matches!(self.state.as_str(), "spawn scheduled" | "waiting")
            && self.job_state == "spawn failed"
            && self.exit.split(':').next() == Some("78")
    }
    pub fn diagnostic(&self) -> String {
        format!(
            "launchd：状态 {}，作业 {}，登记版本 {}，退出码 {}，PID {}",
            self.state,
            self.job_state,
            self.version,
            self.exit,
            self.pid.map_or_else(|| "无".into(), |pid| pid.to_string())
        )
    }
}

#[cfg(target_os = "macos")]
pub(super) fn inspect() -> Result<Option<Job>, String> {
    let output = std::process::Command::new("/bin/launchctl")
        .args(["print", SERVICE])
        .output()
        .map_err(|e| format!("无法查询网络 helper：{e}"))?;
    if !output.status.success() {
        return Err("launchd 未返回可核实的网络 helper 登记".into());
    }
    if output.stdout.len() > 64 * 1024 {
        return Err("网络 helper 登记信息超限".into());
    }
    Ok(std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(Job::parse))
}

#[cfg(test)]
mod tests {
    use super::*;
    const FAILED: &str = "system/com.bgasol.singshell.network = {\n\tactive count = 0\n\tmanaged_by = com.apple.xpc.ServiceManagement\n\tstate = spawn scheduled\n\tprogram identifier = Contents/MacOS/sing-shell (mode: 2)\n\tparent bundle identifier = com.bgasol.singshell\n\tparent bundle version = 0.5.1\n\tlast exit code = 78: EX_CONFIG\n\tsockets = {\n\t\tControl = {\n\t\t\tactive count = 1\n\t\t}\n\t}\n\tjob state = spawn failed\n}\n";
    #[test]
    fn user_report_identifies_only_the_failed_inactive_owned_job() {
        let job = Job::parse(FAILED).unwrap();
        assert!(job.failed_inactive_registration());
        assert!(job.diagnostic().contains("0.5.1"));
        for (from, to) in [
            ("active count = 0", "active count = 1"),
            ("state = spawn scheduled", "state = running"),
            ("job state = spawn failed", "job state = running"),
            ("78: EX_CONFIG", "1"),
            ("com.apple.xpc.ServiceManagement", "another.manager"),
            (
                "parent bundle identifier = com.bgasol.singshell",
                "parent bundle identifier = another.app",
            ),
            ("Contents/MacOS/sing-shell (mode: 2)", "another/program"),
        ] {
            assert!(
                !Job::parse(&FAILED.replace(from, to))
                    .unwrap()
                    .failed_inactive_registration(),
                "{from}"
            );
        }
        assert!(
            !Job::parse(&FAILED.replace("\tjob state", "\tpid = 123\n\tjob state"))
                .unwrap()
                .failed_inactive_registration()
        );
    }
    #[test]
    fn missing_ambiguous_or_unknown_registration_never_authorizes_a_reset() {
        assert!(Job::parse(
            &FAILED.replace("system/com.bgasol.singshell.network", "system/another")
        )
        .is_none());
        assert!(
            Job::parse(&FAILED.replace("\tjob state", "\tactive count = 0\n\tjob state")).is_none()
        );
        assert!(!Job::parse(&FAILED.replace("\tactive count = 0\n", ""))
            .unwrap()
            .failed_inactive_registration());
        assert!(Job::parse(FAILED.trim_end().trim_end_matches('}')).is_none());
    }
}
