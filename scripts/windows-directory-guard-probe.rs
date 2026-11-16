//! Diagnostic only: rustc --edition=2024 this file on Windows. No acceptance waiver.
//! Masks: 0x80000000 existing GENERIC_READ; 0xa0 documented TRAVERSE|READ_ATTRIBUTES.
//! Cases: 1/2 same-dir null/retained root; 3 cross-dir; 4/5 replace or backup/restore;
//! 10..14 empty and 20..24 populated write/append/delete/path/native-rename attempts;
//! 30 junction setup; 31/34 reject final/ancestor reparse; 32/33 reject retarget;
//! 90..99 setup failures. Every output value is a boolean or integer, never a path.
#![cfg(windows)]
use std::{
    ffi::c_void,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    ptr::null_mut,
    time::{SystemTime, UNIX_EPOCH},
};
const FLAGS: u32 = 0x0220_0000; // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
#[repr(C)]
union Status {
    code: i32,
    pointer: *mut c_void,
}
#[repr(C)]
struct IoStatus {
    status: Status,
    information: usize,
}
#[repr(C)]
struct Rename {
    replace: u32,
    root: *mut c_void,
    length: u32,
    name: [u16; 1],
}
// ABI checks for both 64-bit Windows runners (also valid for 32-bit layout).
const _: () = {
    assert!(size_of::<IoStatus>() == 2 * size_of::<usize>());
    assert!(std::mem::offset_of!(IoStatus, information) == size_of::<usize>());
    assert!(std::mem::offset_of!(Rename, root) == size_of::<usize>());
    assert!(std::mem::offset_of!(Rename, name) == 2 * size_of::<usize>() + 4);
};
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSetInformationFile(
        file: *mut c_void,
        io: *mut IoStatus,
        data: *const c_void,
        len: u32,
        class: i32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn DeviceIoControl(
        file: *mut c_void,
        code: u32,
        data: *const c_void,
        len: u32,
        out: *mut c_void,
        out_len: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}
fn emit(mask: u32, case: u32, result: io::Result<()>, expected: bool) {
    println!(
        "{{\"mask\":{mask},\"case\":{case},\"success\":{},\"expected\":{expected},\"os_error\":{}}}",
        result.is_ok(),
        result.err().map_or(0, |e| e.raw_os_error().unwrap_or(-1))
    );
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("kiln-guard-probe-{}-{stamp}", std::process::id()));
        fs::create_dir(&path)?;
        let mut temp = Self(path);
        temp.0 = temp.0.canonicalize()?;
        Ok(temp)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let result = fs::remove_dir_all(&self.0);
        println!(
            "{{\"cleanup_ok\":{},\"os_error\":{}}}",
            result.is_ok(),
            result.err().map_or(0, |e| e.raw_os_error().unwrap_or(-1))
        );
    }
}
fn open(path: &Path, access: u32, sharing: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(sharing)
        .custom_flags(FLAGS)
        .open(path)
}
fn guards(path: &Path, mask: u32) -> io::Result<Vec<File>> {
    let mut result = Vec::new();
    for part in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let file = open(part, mask, 1)?;
        let meta = file.metadata()?;
        if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("reparse or non-directory"));
        }
        result.push(file);
    }
    Ok(result)
}
fn rename(source: &Path, destination: Option<&File>, name: &str) -> io::Result<()> {
    let file = open(source, 0x10000, 1)?; // DELETE; no OVERLAPPED: synchronous lifetime
    let name = name.encode_utf16().collect::<Vec<_>>();
    let bytes = size_of::<Rename>() + (name.len() + 1) * 2;
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<Rename>();
    let mut io = IoStatus {
        status: Status {
            pointer: null_mut(),
        },
        information: 0,
    };
    // repr(C) mirrors FILE_RENAME_INFORMATION and IO_STATUS_BLOCK. The union's
    // ReplaceIfExists byte is one, remaining bytes zero. Storage is handle-aligned.
    let status = unsafe {
        (*info).replace = 1;
        (*info).root = destination.map_or(null_mut(), |f| f.as_raw_handle());
        (*info).length = (name.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).name).cast(),
            name.len(),
        );
        NtSetInformationFile(file.as_raw_handle(), &mut io, info.cast(), bytes as u32, 10)
    };
    if status < 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ))
    } else {
        Ok(())
    }
}
fn write(path: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(b"inert")?;
    file.sync_all()
}
fn positive(mask: u32, form: u32) -> io::Result<()> {
    let temp = Temp::new()?;
    let a = temp.0.join("a");
    let b = temp.0.join("b");
    fs::create_dir(&a)?;
    fs::create_dir(&b)?;
    let ga = guards(&a, mask)?;
    let gb = guards(&b, mask)?;
    let target = if form == 3 { &b } else { &a };
    let handle = match form {
        1 => None,
        2 => ga.last(),
        _ => gb.last(),
    };
    if form == 3 {
        fs::create_dir(a.join("stage"))?;
        write(&a.join("stage/asset"))?;
    } else {
        write(&a.join("stage"))?;
    }
    let result = rename(&a.join("stage"), handle, "published");
    let succeeded = result.is_ok();
    emit(mask, form, result, true);
    if succeeded {
        if form == 3 {
            let result = rename(&b.join("published"), ga.last(), "backup");
            let backed_up = result.is_ok();
            emit(mask, 4, result, true);
            if backed_up {
                emit(
                    mask,
                    5,
                    rename(&a.join("backup"), gb.last(), "published"),
                    true,
                );
            }
        } else {
            write(&a.join("replacement"))?;
            emit(
                mask,
                4,
                rename(&a.join("replacement"), handle, "published"),
                true,
            );
        }
        emit(
            mask,
            6,
            fs::read(if form == 3 {
                target.join("published/asset")
            } else {
                target.join("published")
            })
            .and_then(|b| {
                if b == b"inert" {
                    Ok(())
                } else {
                    Err(io::Error::other("content"))
                }
            }),
            true,
        );
    }
    Ok(())
}
fn negative(mask: u32, populated: bool) -> io::Result<()> {
    let temp = Temp::new()?;
    for op in 0..5 {
        let held = temp.0.join(format!("held-{op}"));
        fs::create_dir(&held)?;
        if populated {
            write(&held.join("asset"))?;
        }
        let _guards = guards(&held, mask)?;
        let result = match op {
            0..=2 => open(&held, [2, 4, 0x10000][op as usize], 7).map(drop),
            3 => fs::rename(&held, temp.0.join("moved")),
            _ => rename(&held, None, "native-moved"),
        };
        emit(mask, if populated { 20 } else { 10 } + op, result, false);
    }
    Ok(())
}
// Owned junctions use REPARSE_DATA_BUFFER's mount-point layout, not shell commands.
fn junction(path: &Path, target: &Path) -> io::Result<()> {
    let file = open(path, 0x100, 7)?; // WRITE_ATTRIBUTES: test actual retarget ability
    let target = target.as_os_str().encode_wide().collect::<Vec<_>>();
    if target.get(..4) != Some(&[92, 92, 63, 92]) {
        return Err(io::Error::other("non-verbatim fixture"));
    }
    let mut nt = vec![92, 63, 63, 92];
    nt.extend_from_slice(&target[4..]);
    let mut data = Vec::new();
    data.extend_from_slice(&0xa0000003u32.to_le_bytes());
    for v in [
        8 + (nt.len() + 2) * 2,
        0,
        0,
        nt.len() * 2,
        (nt.len() + 1) * 2,
        0,
    ] {
        data.extend_from_slice(&(v as u16).to_le_bytes());
    }
    for v in nt.into_iter().chain([0, 0]) {
        data.extend_from_slice(&v.to_le_bytes());
    }
    let mut returned = 0;
    // Synchronous handle; byte buffer and returned count remain live through completion.
    if unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x900a4,
            data.as_ptr().cast(),
            data.len() as u32,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn reparses(mask: u32) -> io::Result<()> {
    let temp = Temp::new()?;
    let target = temp.0.join("target");
    let link = temp.0.join("link");
    fs::create_dir(&target)?;
    fs::create_dir(target.join("child"))?;
    fs::create_dir(&link)?;
    let made = junction(&link, &target);
    let ready = made.is_ok();
    emit(mask, 30, made, true);
    if !ready {
        for case in [31, 32, 33, 34] {
            println!("{{\"mask\":{mask},\"case\":{case},\"inconclusive\":true}}");
        }
        return Ok(());
    }
    emit(mask, 31, guards(&link, mask).map(drop), false);
    emit(mask, 34, guards(&link.join("child"), mask).map(drop), false);
    for populated in [false, true] {
        let held = temp.0.join(if populated { "full" } else { "empty" });
        fs::create_dir(&held)?;
        if populated {
            write(&held.join("asset"))?;
        }
        let _guards = guards(&held, mask)?;
        emit(
            mask,
            if populated { 33 } else { 32 },
            junction(&held, &target),
            false,
        );
    }
    Ok(())
}
fn main() {
    for mask in [0x80000000, 0xa0] {
        for form in 1..=3 {
            if let Err(e) = positive(mask, form) {
                emit(mask, 90 + form, Err(e), true);
            }
        }
        for full in [false, true] {
            if let Err(e) = negative(mask, full) {
                emit(mask, 98, Err(e), true);
            }
        }
        if let Err(e) = reparses(mask) {
            emit(mask, 99, Err(e), true);
        }
    }
}
