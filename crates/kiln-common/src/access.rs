//! Folders that only the current Windows user can open.
#![cfg(windows)]
use std::{io, path::Path};

/// Limits `path` to the current user: a protected DACL whose one entry gives the user full
/// control, inherited by everything created inside.
pub fn restrict_to_user(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW};
    let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{})", user_sid()?).encode_utf16().chain([0]).collect();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: both strings are NUL-terminated; the descriptor is freed with LocalFree.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
            return Err(io::Error::last_os_error());
        }
        let ok = SetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor) != 0;
        let error = io::Error::last_os_error();
        LocalFree(descriptor);
        if ok { Ok(()) } else { Err(error) }
    }
}

/// The current user's security identifier in its `S-1-…` form.
fn user_sid() -> io::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: the token handle is checked and closed once; the buffer is sized by the first
    // query and aligned for TOKEN_USER; the SID string is freed with LocalFree.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut needed = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), needed, &mut needed) != 0;
        CloseHandle(token);
        if !ok {
            return Err(io::Error::last_os_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let len = (0..).take_while(|&n| *text.add(n) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(sid)
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, GetFileSecurityW, PSECURITY_DESCRIPTOR};

    /// The DACL of `descriptor` in SDDL, with well-known accounts by their aliases.
    ///
    /// # Safety
    /// `descriptor` points to a valid security descriptor.
    unsafe fn sddl_of(descriptor: PSECURITY_DESCRIPTOR) -> String {
        let mut text = std::ptr::null_mut();
        // SAFETY: the caller passes a valid descriptor; the returned string is freed with LocalFree.
        unsafe {
            assert!(ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, std::ptr::null_mut()) != 0);
            let len = (0..).take_while(|&n| *text.add(n) != 0).count();
            let out = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            out
        }
    }

    fn dacl_text(path: &std::path::Path) -> String {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut needed = 0u32;
        // SAFETY: a zero-length query only reports the required size.
        unsafe { GetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION, std::ptr::null_mut(), 0, &mut needed) };
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: `buf` holds `needed` bytes of the descriptor the query fills.
        unsafe {
            assert!(GetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION, buf.as_mut_ptr().cast(), needed, &mut needed) != 0);
            sddl_of(buf.as_mut_ptr().cast())
        }
    }

    /// `sddl` as Windows writes it back, so an account with an alias (such as `LA` for the
    /// built-in Administrator) reads the same as in `dacl_text`.
    fn normalized(sddl: &str) -> String {
        let wide: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: the string is NUL-terminated; the descriptor is freed with LocalFree.
        unsafe {
            assert!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) != 0);
            let out = sddl_of(descriptor);
            LocalFree(descriptor);
            out
        }
    }

    #[test]
    fn a_restricted_folder_admits_only_the_current_user_and_passes_that_on() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("private");
        std::fs::create_dir(&folder).unwrap();
        super::restrict_to_user(&folder).unwrap();
        let entry = normalized(&format!("D:(A;OICI;FA;;;{})", super::user_sid().unwrap()))["D:".len()..].to_owned();
        let account = entry.trim_end_matches(')').rsplit(';').next().unwrap().to_owned();
        let text = dacl_text(&folder);
        assert!(text.starts_with("D:P"), "{text}");
        assert_eq!(text.matches('(').count(), 1, "{text}");
        assert!(text.contains(&entry), "{text} lacks {entry}");
        std::fs::create_dir(folder.join("child")).unwrap();
        let child = dacl_text(&folder.join("child"));
        assert_eq!(child.matches('(').count(), 1, "{child}");
        assert!(child.contains(&format!(";{account})")), "{child} lacks {account}");
    }
}
