use std::{
    io,
    process::{Child, Command},
};

#[cfg(not(windows))]
pub struct Guard;
#[cfg(not(windows))]
impl Guard {
    pub fn attach(_: &Child) -> io::Result<Self> {
        Ok(Self)
    }
}

#[cfg(windows)]
pub struct Guard {
    _handle: std::os::windows::io::OwnedHandle,
}
#[cfg(windows)]
impl Guard {
    pub fn attach(child: &Child) -> io::Result<Self> {
        use std::{
            mem::{size_of, zeroed},
            os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        };
        use windows_sys::Win32::System::JobObjects::*;
        // The anonymous, non-inheritable handle belongs only to this parent.
        // Dropping it (including process exit) terminates the contained worker.
        unsafe {
            let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let owned = OwnedHandle::from_raw_handle(raw);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
                || AssignProcessToJobObject(raw, child.as_raw_handle()) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { _handle: owned })
        }
    }
}

pub fn configure(command: &mut Command) {
    command.env_clear();
    // No API keys, transcript, dictionary or arbitrary environment inheritance.
    // Windows needs its OS directory to locate system libraries.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
}
