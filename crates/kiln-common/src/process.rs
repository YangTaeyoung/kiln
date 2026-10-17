//! Helper processes started without a visible window, and Windows Job Objects.
use std::process::Command;

/// Starts `cmd` without a console window on Windows (`CREATE_NO_WINDOW`); other systems
/// are unchanged.
pub fn hide_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A Windows Job Object holding a process and every process it starts afterwards.
#[cfg(windows)]
pub struct Job(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a job handle may be used and closed from any thread.
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    /// A job holding the process `pid`. With `end_on_close`, closing the job ends every
    /// process in it.
    pub fn for_process(pid: u32, end_on_close: bool) -> Option<Job> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
        // SAFETY: every handle is checked before use and closed once; the limit structure
        // matches the information class.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let job = Job(job);
            if end_on_close {
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
                if SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), size) == 0 {
                    return None;
                }
            }
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process.is_null() {
                return None;
            }
            let assigned = AssignProcessToJobObject(job.0, process) != 0;
            CloseHandle(process);
            assigned.then_some(job)
        }
    }

    /// Ends every process in the job.
    pub fn terminate(&self) {
        // SAFETY: the job handle is open for the lifetime of `self`.
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) };
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by `self` and closed once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    fn waiting() -> Child {
        let mut cmd = Command::new("ping");
        super::hide_window(&mut cmd).args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null());
        cmd.spawn().unwrap()
    }

    fn ends_soon(child: &mut Child) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn closing_a_job_that_ends_on_close_ends_its_processes() {
        let mut child = waiting();
        drop(super::Job::for_process(child.id(), true).unwrap());
        assert!(ends_soon(&mut child));
    }

    #[test]
    fn closing_a_plain_job_leaves_its_processes_and_terminate_ends_them() {
        let mut child = waiting();
        drop(super::Job::for_process(child.id(), false).unwrap());
        assert!(!ends_soon(&mut child));
        super::Job::for_process(child.id(), false).unwrap().terminate();
        assert!(ends_soon(&mut child));
    }
}
