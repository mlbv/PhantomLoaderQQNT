// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::os::windows::ffi::OsStringExt;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, GetFileAttributesW, INVALID_FILE_ATTRIBUTES},
    System::SystemInformation::GetSystemDirectoryW,
};

use crate::{
    Result,
    process::{SuspendedProcess, full_path, start_detached, wide, windows_error},
};

pub struct Reporter {
    script: PathBuf,
    capture_directory: PathBuf,
    powershell: PathBuf,
}

impl Reporter {
    pub fn configure(script: &Path, capture_directory: &Path) -> Result<Self> {
        let script = validate_path(script, false)?;
        let capture_directory = validate_path(capture_directory, true)?;
        let mut system = vec![0; 32768];
        let length =
            unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
        if length == 0 || length >= system.len() {
            return Err(windows_error("GetSystemDirectoryW"));
        }
        let powershell = PathBuf::from(OsString::from_wide(&system[..length]))
            .join("WindowsPowerShell/v1.0/powershell.exe");
        let path = wide(powershell.as_os_str())?;
        let attributes = unsafe { GetFileAttributesW(path.as_ptr()) };
        if attributes == INVALID_FILE_ATTRIBUTES || attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(windows_error("GetFileAttributesW(Windows PowerShell)"));
        }
        Ok(Self {
            script,
            capture_directory,
            powershell,
        })
    }

    pub fn monitor(&self, process: &SuspendedProcess) -> Result<u32> {
        let code = process.wait_for_exit()?;
        if should_report(code) {
            let (created, exited) = process.times()?;
            start_detached(
                &self.powershell,
                &self.arguments(process.id, code, created, exited),
                self.script.parent().unwrap(),
            )
            .map_err(|error| format!("CreateProcessW(capture reporter): {error}"))?;
        }
        Ok(code)
    }

    pub fn arguments(
        &self,
        process_id: u32,
        exit_code: u32,
        created: u64,
        exited: u64,
    ) -> Vec<OsString> {
        [
            OsString::from("-NoProfile"),
            OsString::from("-ExecutionPolicy"),
            OsString::from("Bypass"),
            OsString::from("-STA"),
            OsString::from("-WindowStyle"),
            OsString::from("Hidden"),
            OsString::from("-File"),
            self.script.as_os_str().to_owned(),
            OsString::from("-CaptureDirectory"),
            self.capture_directory.as_os_str().to_owned(),
            OsString::from("-ProcessId"),
            OsString::from(process_id.to_string()),
            OsString::from("-ExitCode"),
            OsString::from(exit_code.to_string()),
            OsString::from("-ProcessStartedFiletime"),
            OsString::from(created.to_string()),
            OsString::from("-ProcessExitedFiletime"),
            OsString::from(exited.to_string()),
        ]
        .into()
    }
}

pub fn should_report(exit_code: u32) -> bool {
    platform::incident::needs_report(exit_code)
}

fn validate_path(path: &Path, directory: bool) -> Result<PathBuf> {
    if !path.is_absolute()
        || !directory
            && !path.extension().is_some_and(|extension| {
                extension
                    .to_str()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
            })
    {
        return Err(if directory {
            "-capture-directory requires an absolute directory path"
        } else {
            "-reporter requires an absolute .ps1 script path"
        }
        .into());
    }
    let path = full_path(path)?;
    let encoded = wide(path.as_os_str())?;
    let attributes = unsafe { GetFileAttributesW(encoded.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(windows_error(if directory {
            "GetFileAttributesW(capture directory)"
        } else {
            "GetFileAttributesW(reporter)"
        }));
    }
    if (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory {
        return Err(if directory {
            "Capture path must be a directory"
        } else {
            "Reporter path must be a script file"
        }
        .into());
    }
    Ok(path)
}
