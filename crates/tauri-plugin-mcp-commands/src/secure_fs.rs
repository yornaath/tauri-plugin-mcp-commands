//! Owner-only files: mode 0700/0600 on Unix, a protected DACL granting only
//! the current user on Windows.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Creates `dir` (and parents) and restricts it to the current user.
/// Refuses symlinks, so a planted link can't redirect writes.
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io::Error::other(format!("{} is a symlink; refusing to use it", dir.display())));
        }
        Ok(meta) if !meta.is_dir() => {
            return Err(io::Error::other(format!("{} exists and is not a directory", dir.display())));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir_all(dir)?,
        Err(e) => return Err(e),
    }
    restrict(dir, true)
}

/// Atomically replaces `path` with `contents`, owner-only from the moment the
/// file exists: a fresh temp file is created next to it, locked down, written,
/// synced and renamed over the target.
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let tmp = temp_path(path);
    let mut file = create_new_private(&tmp)?;
    let result = (|| {
        restrict(&tmp, false)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let mut nonce = [0u8; 8];
    let _ = getrandom::fill(&mut nonce);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.tmp", u64::from_le_bytes(nonce)))
}

#[cfg(unix)]
fn create_new_private(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn create_new_private(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(unix)]
fn restrict(path: &Path, is_dir: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(if is_dir { 0o700 } else { 0o600 }))
}

#[cfg(windows)]
fn restrict(path: &Path, is_dir: bool) -> io::Result<()> {
    windows::set_owner_only(path, is_dir)
}

#[cfg(not(any(unix, windows)))]
fn restrict(_: &Path, _: bool) -> io::Result<()> {
    Ok(())
}

/// Whether only the current user can access `path`.
pub fn is_owner_only(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = fs::symlink_metadata(path)?;
        // SAFETY: geteuid has no preconditions.
        let euid = unsafe { libc_geteuid() };
        Ok(meta.uid() == euid && meta.permissions().mode() & 0o077 == 0)
    }
    #[cfg(windows)]
    {
        windows::is_owner_only(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "geteuid"]
    fn libc_geteuid() -> u32;
}

#[cfg(windows)]
mod windows {
    use std::{ffi::c_void, io, os::windows::ffi::OsStrExt, path::Path, ptr};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, AddAccessAllowedAceEx, Authorization::{
                GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
            }, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetLengthSid,
            GetSecurityDescriptorControl, GetTokenInformation, InitializeAcl, OBJECT_INHERIT_ACE,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
            TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// The current user's SID, owned in a buffer.
    struct UserSid(Vec<u8>);

    impl UserSid {
        fn current() -> io::Result<Self> {
            // SAFETY: plain Win32 calls with valid out-pointers; the token
            // handle is closed on every path.
            unsafe {
                let mut token: HANDLE = ptr::null_mut();
                if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut len = 0u32;
                GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
                let mut buf = vec![0u8; len as usize];
                let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
                CloseHandle(token);
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self(buf))
            }
        }

        fn sid(&self) -> *mut c_void {
            // SAFETY: the buffer holds a TOKEN_USER written by GetTokenInformation.
            unsafe { (*(self.0.as_ptr() as *const TOKEN_USER)).User.Sid }
        }
    }

    pub(super) fn set_owner_only(path: &Path, is_dir: bool) -> io::Result<()> {
        let user = UserSid::current()?;
        // SAFETY: the ACL buffer is sized for one ACE carrying this SID and
        // u32-aligned; all pointers outlive the calls.
        unsafe {
            let sid_len = GetLengthSid(user.sid());
            let size = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>() + sid_len as usize;
            let mut acl_buf = vec![0u32; size.div_ceil(4)];
            let acl = acl_buf.as_mut_ptr() as *mut ACL;
            if InitializeAcl(acl, (acl_buf.len() * 4) as u32, ACL_REVISION) == 0 {
                return Err(io::Error::last_os_error());
            }
            let inherit = if is_dir { OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE } else { 0 };
            if AddAccessAllowedAceEx(acl, ACL_REVISION, inherit, GENERIC_ALL, user.sid()) == 0 {
                return Err(io::Error::last_os_error());
            }
            let path = wide(path);
            let err = SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null(),
            );
            if err != 0 {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
        }
        Ok(())
    }

    /// True when the DACL is protected (no inherited entries) and only allows
    /// the current user.
    pub(super) fn is_owner_only(path: &Path) -> io::Result<bool> {
        let user = UserSid::current()?;
        let path = wide(path);
        // SAFETY: GetNamedSecurityInfoW allocates `sd`, which is freed below;
        // `dacl` points into it.
        unsafe {
            let mut dacl: *mut ACL = ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
            let err = GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut sd,
            );
            if err != 0 {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            let mut control = 0u16;
            let mut revision = 0u32;
            GetSecurityDescriptorControl(sd, &mut control, &mut revision);
            // Directories get two entries for the user: the effective one and
            // an inherit-only one for children. Every entry must be an allow
            // entry for the current user.
            let mut ok = control & SE_DACL_PROTECTED != 0 && !dacl.is_null() && (*dacl).AceCount > 0;
            for i in 0..if ok { (*dacl).AceCount as u32 } else { 0 } {
                let mut ace: *mut c_void = ptr::null_mut();
                ok &= GetAce(dacl, i, &mut ace) != 0 && {
                    let ace = ace as *const ACCESS_ALLOWED_ACE;
                    // ACCESS_ALLOWED_ACE_TYPE is 0.
                    (*ace).Header.AceType == 0
                        && EqualSid(ptr::addr_of!((*ace).SidStart) as *mut c_void, user.sid()) != 0
                };
            }
            LocalFree(sd as _);
            Ok(ok)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dir_and_file() {
        let base = std::env::temp_dir().join(format!("mcp-secure-fs-{}", std::process::id()));
        let dir = base.join("nested");
        create_private_dir(&dir).unwrap();
        assert!(is_owner_only(&dir).unwrap());

        let file = dir.join("secret.json");
        write_private(&file, b"one").unwrap();
        write_private(&file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        assert!(is_owner_only(&file).unwrap());
        // No temp files left behind.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        // An ordinary file (inherited ACL / default umask) is not owner-only.
        let plain = base.join("plain.txt");
        fs::write(&plain, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&plain, fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(!is_owner_only(&plain).unwrap());

        fs::remove_dir_all(&base).unwrap();
    }
}
