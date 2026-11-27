use std::{
    io,
    process::{Child, Command},
};
pub(crate) mod record;

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::{
                OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
            },
        },
    };

    pub struct ProcessGroup(OwnedHandle, u32, bool);
    static NETWORK_WORKER: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    pub fn network_worker() -> io::Result<()> {
        use windows_sys::Win32::{
            System::Console::{AllocConsole, GetConsoleWindow},
            UI::WindowsAndMessaging::{ShowWindow, SW_HIDE},
        };
        unsafe {
            if GetConsoleWindow().is_null() && AllocConsole() == 0 {
                return Err(io::Error::last_os_error());
            }
            let console = GetConsoleWindow();
            if !console.is_null() {
                ShowWindow(console, SW_HIDE);
            }
        }
        NETWORK_WORKER.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
        }
    }

    fn resume(child: &Child) -> io::Result<()> {
        let snapshot = owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
        while found {
            if entry.th32OwnerProcessID == child.id() {
                let thread =
                    owned(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
                return if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                };
            }
            found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
        }
        Err(io::Error::other("无法恢复 sing-box 进程"))
    }

    pub fn spawn(command: &mut Command) -> io::Result<(Child, ProcessGroup)> {
        let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let network = NETWORK_WORKER.load(std::sync::atomic::Ordering::Acquire);
        let flags = if network {
            windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP
        } else {
            CREATE_NO_WINDOW
        };
        let mut child = command.creation_flags(flags | CREATE_SUSPENDED).spawn()?;
        let group = ProcessGroup(job, child.id(), network);
        let assigned =
            unsafe { AssignProcessToJobObject(group.0.as_raw_handle(), child.as_raw_handle()) }
                != 0;
        let result = if assigned {
            resume(&child)
        } else {
            Err(io::Error::last_os_error())
        };
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, group))
    }

    impl ProcessGroup {
        pub fn request_shutdown(&self) -> io::Result<bool> {
            if !self.2 {
                return Ok(false);
            }
            if unsafe {
                windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent(
                    windows_sys::Win32::System::Console::CTRL_BREAK_EVENT,
                    self.1,
                )
            } == 0
            {
                Err(io::Error::last_os_error())
            } else {
                Ok(true)
            }
        }
        pub fn terminate(&self) -> io::Result<()> {
            if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
mod native {
    use super::*;
    use std::os::unix::process::CommandExt;

    pub struct ProcessGroup(i32);

    pub fn spawn(command: &mut Command) -> io::Result<(Child, ProcessGroup)> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let child = loop {
            match command.process_group(0).spawn() {
                Ok(child) => break child,
                // Concurrent forks can briefly retain a prepared executable's write descriptor.
                Err(e)
                    if e.raw_os_error() == Some(libc::ETXTBSY)
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        };
        let group = ProcessGroup(child.id() as i32);
        Ok((child, group))
    }

    impl ProcessGroup {
        pub fn request_shutdown(&self) -> io::Result<bool> {
            if unsafe { libc::kill(-self.0, libc::SIGTERM) } == 0 {
                Ok(true)
            } else {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(false)
                } else {
                    Err(error)
                }
            }
        }
        pub fn terminate(&self) -> io::Result<()> {
            if unsafe { libc::kill(-self.0, libc::SIGKILL) } == 0 {
                Ok(())
            } else {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
    #[cfg(all(test, target_os = "linux"))]
    #[test]
    fn executable_busy_during_write_is_retried_after_writer_closes() {
        let root = std::env::temp_dir().join(format!(
            "sing-shell-textbusy-{}-{}",
            std::process::id(),
            crate::logs::now()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("probe");
        std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        let writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            drop(writer);
        });
        let (mut child, _group) = spawn(
            Command::new(&path)
                .arg("--list")
                .stdout(std::process::Stdio::null()),
        )
        .unwrap();
        assert!(child.wait().unwrap().success());
        release.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
pub use native::network_worker;
pub use native::{spawn, ProcessGroup};
#[cfg(unix)]
pub fn network_worker() -> io::Result<()> {
    Ok(())
}
