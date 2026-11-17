//! A private job for an agent host invoked inside an ordinary PowerShell PTY.
//!
//! Claim before spawning any provider/helper. The parent shell remains only in
//! the daemon's outer PTY job; this unnamed nested job contains the host and its
//! future children. Never query a NULL/default job here, enable breakaway, or
//! enable KILL_ON_JOB_CLOSE. Drop closes the capability and kills no process.
#![cfg(windows)]
use anyhow::{Context, Result, ensure};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JobObjectBasicProcessIdList, QueryInformationJobObject, TerminateJobObject,
        },
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
        },
    },
};

const MAX_PROCESSES: usize = 4096;
#[repr(C)]
struct ProcessList {
    assigned: u32,
    listed: u32,
    pids: [usize; MAX_PROCESSES],
}

pub(super) struct OwnedJob {
    handle: OwnedHandle,
    owner: u32,
}
impl OwnedJob {
    /// A new empty job can nest an already-jobbed process on Windows 8+ only
    /// when the parent hierarchy permits it (in particular, no UI limits).
    /// Return an error without altering the parent's job/settings on failure.
    pub(super) fn claim_current_process() -> Result<Self> {
        // NULL security attributes make the handle non-inheritable. NULL name
        // creates a new private job, never an existing shared/daemon job.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        ensure!(
            !raw.is_null(),
            "Could not create private agent job: {}",
            std::io::Error::last_os_error()
        );
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        // A new job has no limits. Do not set limits or termination policies on
        // this job or query/change any ancestor job to make nesting succeed.
        let assigned = unsafe { AssignProcessToJobObject(raw, GetCurrentProcess()) };
        ensure!(
            assigned != 0,
            "Could not nest private agent job: {}",
            std::io::Error::last_os_error()
        );
        let job = Self {
            handle,
            owner: unsafe { GetCurrentProcessId() },
        };
        ensure!(
            job.inspect_children()?.is_empty(),
            "Private agent job was not claimed before child launch"
        );
        Ok(job)
    }
    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
    fn verify_owner(&self) -> Result<()> {
        ensure!(
            unsafe { GetCurrentProcessId() } == self.owner,
            "Private agent job owner changed"
        );
        let mut belongs = 0;
        let result = unsafe { IsProcessInJob(GetCurrentProcess(), self.raw(), &mut belongs) };
        ensure!(
            result != 0 && belongs != 0,
            "Host is absent from its private agent job"
        );
        Ok(())
    }
    /// Complete current membership of this explicit job and its child jobs,
    /// excluding the host. Unknown/truncated membership fails closed.
    pub(super) fn inspect_children(&self) -> Result<Vec<u32>> {
        self.verify_owner()?;
        let mut list = ProcessList {
            assigned: 0,
            listed: 0,
            pids: [0; MAX_PROCESSES],
        };
        let result = unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicProcessIdList,
                (&mut list as *mut ProcessList).cast(),
                std::mem::size_of::<ProcessList>() as u32,
                std::ptr::null_mut(),
            )
        };
        ensure!(
            result != 0,
            "Could not inspect private agent job: {}",
            std::io::Error::last_os_error()
        );
        checked_children(&list, self.owner)
    }
    /// Explicit shutdown cleanup, never a presentation-switch authorization.
    /// Each opened process handle is rechecked against this private job before
    /// termination, preventing a recycled PID from targeting another process.
    pub(super) fn cleanup_children(&self) -> Result<()> {
        self.verify_owner()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let children = self.inspect_children()?;
            if children.is_empty() {
                return Ok(());
            }
            for pid in children {
                let raw = unsafe {
                    OpenProcess(
                        PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                        0,
                        pid,
                    )
                };
                if raw.is_null() {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                        continue;
                    }
                    return Err(error).context("Could not open owned agent child for cleanup");
                }
                let process = unsafe { OwnedHandle::from_raw_handle(raw) };
                let mut belongs = 0;
                let queried =
                    unsafe { IsProcessInJob(process.as_raw_handle(), self.raw(), &mut belongs) };
                ensure!(
                    queried != 0,
                    "Could not recheck private agent child membership"
                );
                if belongs == 0 {
                    continue;
                }
                if unsafe { TerminateProcess(process.as_raw_handle(), 1) } == 0 {
                    let error = std::io::Error::last_os_error();
                    // The process can naturally exit between inspection and cleanup.
                    ensure!(
                        unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } == WAIT_OBJECT_0,
                        "Could not stop private agent child: {error}"
                    );
                }
            }
            ensure!(
                Instant::now() < deadline,
                "Private agent children did not finish cleanup"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// Explicit final shutdown ONLY, after durable journal flush. This terminates
    /// the host itself and this private subtree; ancestor jobs/shells are outside
    /// the handle. It is never called by Drop or when choosing GUI/TUI tabs.
    pub(super) fn terminate_owned(&self, exit_code: u32) -> Result<()> {
        self.verify_owner()?;
        ensure!(
            unsafe { TerminateJobObject(self.raw(), exit_code) } != 0,
            "Could not terminate private agent job: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
}
fn checked_children(list: &ProcessList, owner: u32) -> Result<Vec<u32>> {
    ensure!(
        list.assigned == list.listed && list.listed as usize <= MAX_PROCESSES,
        "Private agent job membership is incomplete"
    );
    let pids = &list.pids[..list.listed as usize];
    ensure!(
        pids.contains(&(owner as usize)),
        "Private agent host is absent from its job"
    );
    let mut children = Vec::new();
    for pid in pids.iter().copied() {
        let pid = u32::try_from(pid).context("Invalid private agent process ID")?;
        ensure!(pid != 0, "Invalid private agent process ID");
        if pid != owner {
            children.push(pid);
        }
    }
    children.sort_unstable();
    let length = children.len();
    children.dedup();
    ensure!(
        children.len() == length,
        "Duplicate private agent process identity"
    );
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_membership_excludes_only_the_host_and_rejects_missing_or_partial_lists() {
        let mut list = ProcessList {
            assigned: 3,
            listed: 3,
            pids: [0; MAX_PROCESSES],
        };
        list.pids[..3].copy_from_slice(&[30, 10, 20]);
        assert_eq!(checked_children(&list, 10).unwrap(), [20, 30]);
        assert!(checked_children(&list, 40).is_err());
        list.assigned = 4;
        assert!(checked_children(&list, 10).is_err());
        list.assigned = MAX_PROCESSES as u32 + 1;
        list.listed = list.assigned;
        assert!(checked_children(&list, 10).is_err());
    }
    #[test]
    fn invalid_or_duplicate_pids_cannot_authorize_cleanup() {
        let mut list = ProcessList {
            assigned: 3,
            listed: 3,
            pids: [0; MAX_PROCESSES],
        };
        list.pids[..3].copy_from_slice(&[10, 20, 20]);
        assert!(checked_children(&list, 10).is_err());
        list.pids[2] = 0;
        assert!(checked_children(&list, 10).is_err());
        #[cfg(target_pointer_width = "64")]
        {
            list.pids[2] = u32::MAX as usize + 1;
            assert!(checked_children(&list, 10).is_err());
        }
    }
}
