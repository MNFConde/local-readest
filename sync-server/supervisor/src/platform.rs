//! Child-lifetime plumbing so "the supervisor dies ⇒ both children die" holds
//! even for a hard kill (`taskkill /F`, closing the terminal window) — not
//! just for the graceful Ctrl+C path.
//!
//! Windows: every child is assigned to one Job Object with
//! KILL_ON_JOB_CLOSE. The job handle is intentionally leaked so it closes
//! only when the supervisor process itself exits, at which point the kernel
//! terminates everything in the job. Without this, a hard-killed supervisor
//! leaves a headless node.exe squatting on port 3000.
//!
//! Unix: each child calls setsid() (its own process group, so a group kill
//! cannot sweep the supervisor or the sibling) and sets PR_SET_PDEATHSIG so
//! the kernel SIGKILLs it the moment the supervisor exits.

#[cfg(windows)]
mod imp {
    use std::sync::{Mutex, OnceLock};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    static JOB: OnceLock<usize> = OnceLock::new();
    static LIVE_PIDS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

    fn live_pids() -> &'static Mutex<Vec<u32>> {
        LIVE_PIDS.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Create the kill-on-close job object (idempotent; the handle is never
    /// closed for the lifetime of the process).
    pub fn init() {
        JOB.get_or_init(|| {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    // Not fatal: children then rely on the graceful path only.
                    eprintln!("supervisor: CreateJobObjectW failed — hard-kill cleanup degraded");
                    return 0usize;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    eprintln!(
                        "supervisor: SetInformationJobObject failed — hard-kill cleanup degraded"
                    );
                    return 0usize;
                }
                job as usize
            }
        });
    }

    /// Put a freshly spawned child into the job object.
    pub fn assign_child(child: &tokio::process::Child) {
        let job = *JOB.get().unwrap_or(&0);
        if job == 0 {
            return;
        }
        let Some(handle) = child.raw_handle() else {
            return;
        };
        unsafe {
            if AssignProcessToJobObject(job as *mut core::ffi::c_void, handle) == 0 {
                eprintln!("supervisor: AssignProcessToJobObject failed — this child may survive a hard kill");
            }
        }
    }

    pub fn register_pid(pid: u32) {
        live_pids()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(pid);
    }

    pub fn unregister_pid(pid: u32) {
        live_pids()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p| *p != pid);
    }

    /// Graceful-shutdown belt: TerminateProcess every live child. The job
    /// object covers the hard-kill case; this covers Ctrl+C.
    pub fn terminate_children() {
        let pids = live_pids()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for pid in pids {
            kill_pid(pid);
        }
    }

    /// Kill exactly one child by PID (⑩ manual Stop/Restart: the sibling
    /// service must not even notice). Process-group semantics on Unix are
    /// handled by `terminate_children`; here the pid IS the group leader.
    pub fn kill_pid(pid: u32) {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h != INVALID_HANDLE_VALUE && !h.is_null() {
                TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::sync::{Mutex, OnceLock};

    static LIVE_PIDS: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();

    fn live_pids() -> &'static Mutex<Vec<u32>> {
        LIVE_PIDS.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Detach into a new session + group, and ask the kernel to SIGKILL this
    /// child when the supervisor dies (PR_SET_PDEATHSIG).
    pub fn prepare(cmd: &mut tokio::process::Command) {
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    // EPERM when already a session leader — the common,
                    // harmless case for a re-exec'd child.
                    let err = std::io::Error::last_os_error();
                    if err.raw_os_error() != Some(libc::EPERM) {
                        return Err(err);
                    }
                }
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
                // Race: if the parent died between fork and prctl, the signal
                // was never armed — check and exit instead of lingering.
                if libc::getppid() == 1 {
                    return Err(std::io::Error::other("parent exited before pdeathsig"));
                }
                Ok(())
            });
        }
    }

    pub fn init() {}
    pub fn assign_child(_child: &tokio::process::Child) {}

    pub fn register_pid(pid: u32) {
        live_pids()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(pid);
    }

    pub fn unregister_pid(pid: u32) {
        live_pids()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|p| *p != pid);
    }

    /// Kill each child's whole process group (setsid made each its own group,
    /// so this cannot sweep the supervisor or the sibling).
    pub fn terminate_children() {
        let pids = live_pids()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for pid in pids {
            kill_pid(pid);
        }
    }

    /// Kill exactly one child's process group (⑩ manual Stop/Restart).
    pub fn kill_pid(pid: u32) {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
pub use imp::{assign_child, init, kill_pid, register_pid, terminate_children, unregister_pid};

#[cfg(unix)]
pub use imp::{
    assign_child, init, kill_pid, prepare, register_pid, terminate_children, unregister_pid,
};
