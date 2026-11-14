// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::{
    ffi::{OsStr, OsString, c_void},
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE, WAIT_OBJECT_0},
    Globalization::{CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal},
    Storage::FileSystem::GetFullPathNameW,
    System::{
        Diagnostics::Debug::{CONTEXT, FlushInstructionCache, WriteProcessMemory},
        Environment::{FreeEnvironmentStringsW, GetEnvironmentStringsW},
        LibraryLoader::{GetModuleHandleA, GetProcAddress},
        Memory::{
            MEM_COMMIT, MEM_RESERVE, PAGE_READONLY, PAGE_READWRITE, VirtualAllocEx,
            VirtualProtectEx,
        },
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            CreateRemoteThread, GetExitCodeProcess, GetExitCodeThread, GetProcessTimes, INFINITE,
            OpenProcess, PROCESS_CREATE_THREAD, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION,
            PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE, ResumeThread, STARTUPINFOW,
            TerminateProcess, WaitForSingleObject,
        },
    },
};

use crate::{Result, cli::command_line, image::Payload, target::read_payload};

/// Storage satisfying the Windows x64 thread-context API alignment.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct ThreadContext(pub CONTEXT);

pub struct Handle(OwnedHandle);

impl Handle {
    /// # Safety
    /// The handle must be valid, owned by the caller, and closable with CloseHandle.
    pub unsafe fn from_raw(handle: HANDLE) -> Self {
        Self(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    pub fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}

pub fn windows_error(operation: &str) -> String {
    format!("{operation} failed: {}", std::io::Error::last_os_error())
}

pub fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err("Path or argument contains a NUL character".into());
    }
    value.push(0);
    Ok(value)
}

pub fn full_path(path: &Path) -> Result<PathBuf> {
    let path = wide(path.as_os_str())?;
    let mut buffer = vec![0; 32768];
    let length = unsafe {
        GetFullPathNameW(
            path.as_ptr(),
            buffer.len() as u32,
            buffer.as_mut_ptr(),
            null_mut(),
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(windows_error("GetFullPathNameW"));
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

fn create_process(
    executable: &Path,
    arguments: &[OsString],
    directory: &Path,
    flags: u32,
    environment: Option<&[u16]>,
) -> Result<PROCESS_INFORMATION> {
    let application = wide(executable.as_os_str())?;
    let mut command = wide(&command_line(executable.as_os_str(), arguments))?;
    let directory = wide(directory.as_os_str())?;
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            flags
                | if environment.is_some() {
                    CREATE_UNICODE_ENVIRONMENT
                } else {
                    0
                },
            environment.map_or(null(), |block| block.as_ptr().cast()),
            directory.as_ptr(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(windows_error("CreateProcessW"));
    }
    Ok(process)
}

fn coordinator_environment(pipe: &OsStr) -> Result<Vec<u16>> {
    let name: Vec<u16> = platform::coordinator::ENVIRONMENT_VARIABLE
        .encode_utf16()
        .collect();
    let mut replacement = name.clone();
    replacement.push(b'=' as u16);
    replacement.extend(wide(pipe)?);
    let inherited = unsafe { GetEnvironmentStringsW() };
    if inherited.is_null() {
        return Err(windows_error("GetEnvironmentStringsW"));
    }
    let mut entries = Vec::new();
    unsafe {
        let mut cursor = inherited;
        while *cursor != 0 {
            let start = cursor;
            while *cursor != 0 {
                cursor = cursor.add(1);
            }
            let entry = std::slice::from_raw_parts(start, cursor.offset_from(start) as usize);
            let overridden = entry.len() > name.len()
                && entry[name.len()] == b'=' as u16
                && CompareStringOrdinal(
                    entry.as_ptr(),
                    name.len() as i32,
                    name.as_ptr(),
                    name.len() as i32,
                    1,
                ) == CSTR_EQUAL;
            if !overridden {
                let separator = entry[1..]
                    .iter()
                    .position(|&unit| unit == b'=' as u16)
                    .unwrap()
                    + 1;
                let mut entry = entry.to_vec();
                entry.push(0);
                entries.push((separator, entry));
            }
            cursor = cursor.add(1);
        }
        if FreeEnvironmentStringsW(inherited) == 0 {
            std::process::abort();
        }
    }
    entries.push((name.len(), replacement));
    entries.sort_by(|left, right| {
        let comparison = unsafe {
            CompareStringOrdinal(
                left.1.as_ptr(),
                left.0 as i32,
                right.1.as_ptr(),
                right.0 as i32,
                1,
            )
        };
        match comparison {
            CSTR_LESS_THAN => std::cmp::Ordering::Less,
            CSTR_EQUAL => std::cmp::Ordering::Equal,
            CSTR_GREATER_THAN => std::cmp::Ordering::Greater,
            _ => std::process::abort(),
        }
    });
    let mut block: Vec<u16> = entries.into_iter().flat_map(|(_, entry)| entry).collect();
    block.push(0);
    Ok(block)
}

pub fn start_detached(executable: &Path, arguments: &[OsString], directory: &Path) -> Result<()> {
    let process = create_process(executable, arguments, directory, CREATE_NO_WINDOW, None)?;
    unsafe {
        drop(Handle::from_raw(process.hThread));
        drop(Handle::from_raw(process.hProcess));
    }
    Ok(())
}

pub struct SuspendedProcess {
    pub handle: Handle,
    pub id: u32,
    thread: Option<Handle>,
    terminate_on_drop: bool,
}

impl SuspendedProcess {
    pub fn launch(executable: &Path, arguments: &[OsString]) -> Result<Self> {
        Self::launch_with_environment(executable, arguments, None)
    }

    pub fn launch_coordinated(
        executable: &Path,
        arguments: &[OsString],
        pipe: &OsStr,
    ) -> Result<Self> {
        let environment = coordinator_environment(pipe)?;
        Self::launch_with_environment(executable, arguments, Some(&environment))
    }

    fn launch_with_environment(
        executable: &Path,
        arguments: &[OsString],
        environment: Option<&[u16]>,
    ) -> Result<Self> {
        let process = create_process(
            executable,
            arguments,
            executable.parent().unwrap_or(Path::new("")),
            CREATE_SUSPENDED,
            environment,
        )?;
        Ok(Self {
            handle: unsafe { Handle::from_raw(process.hProcess) },
            id: process.dwProcessId,
            thread: Some(unsafe { Handle::from_raw(process.hThread) }),
            terminate_on_drop: true,
        })
    }

    pub fn inject_payload(&self, path: &Path) -> Result<()> {
        self.with_injection_target(|target| inject_payload(target, path))
    }

    #[cfg(feature = "testhost")]
    pub fn inject_test_dll(&self, path: &Path) -> Result<()> {
        self.with_injection_target(|target| inject_test_dll(target, path))
    }

    fn with_injection_target(&self, inject: impl FnOnce(HANDLE) -> Result<()>) -> Result<()> {
        let target = unsafe {
            OpenProcess(
                PROCESS_CREATE_THREAD
                    | PROCESS_QUERY_INFORMATION
                    | PROCESS_VM_OPERATION
                    | PROCESS_VM_WRITE
                    | PROCESS_VM_READ,
                0,
                self.id,
            )
        };
        if target.is_null() {
            return Err(windows_error("OpenProcess"));
        }
        let target = unsafe { Handle::from_raw(target) };
        inject(target.raw())
    }

    pub fn resume(&mut self) -> Result<()> {
        if unsafe { ResumeThread(self.thread.as_ref().unwrap().raw()) } == u32::MAX {
            return Err(windows_error("ResumeThread"));
        }
        self.terminate_on_drop = false;
        self.thread.take();
        Ok(())
    }

    pub fn wait(&self, milliseconds: u32) -> Result<()> {
        if unsafe { WaitForSingleObject(self.handle.raw(), milliseconds) } != WAIT_OBJECT_0 {
            return Err(windows_error("WaitForSingleObject(target exit)"));
        }
        Ok(())
    }

    pub fn wait_for_exit(&self) -> Result<u32> {
        self.wait(INFINITE)?;
        self.exit_code()
    }

    pub fn exit_code(&self) -> Result<u32> {
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.handle.raw(), &mut code) } == 0 {
            return Err(windows_error("GetExitCodeProcess(target exit)"));
        }
        Ok(code)
    }

    pub fn times(&self) -> Result<(u64, u64)> {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if unsafe {
            GetProcessTimes(
                self.handle.raw(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(windows_error("GetProcessTimes(target)"));
        }
        Ok((filetime_value(created), filetime_value(exited)))
    }

    pub fn terminate(&self, code: u32) {
        unsafe {
            TerminateProcess(self.handle.raw(), code);
        }
    }
}

impl Drop for SuspendedProcess {
    fn drop(&mut self) {
        if self.terminate_on_drop {
            self.terminate(1);
        }
    }
}

pub fn filetime_value(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

fn allocate(target: HANDLE, size: usize, label: &str) -> Result<*mut c_void> {
    let address = unsafe {
        VirtualAllocEx(
            target,
            null(),
            size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if address.is_null() {
        return Err(windows_error(label));
    }
    Ok(address)
}

fn write_remote(target: HANDLE, address: usize, bytes: &[u8], label: &str) -> Result<()> {
    if unsafe {
        WriteProcessMemory(
            target,
            address as *const c_void,
            bytes.as_ptr().cast(),
            bytes.len(),
            null_mut(),
        )
    } == 0
    {
        return Err(windows_error(label));
    }
    Ok(())
}

fn protect_remote(
    target: HANDLE,
    address: usize,
    size: usize,
    protection: u32,
    label: &str,
) -> Result<()> {
    let mut old = 0;
    if unsafe { VirtualProtectEx(target, address as *const c_void, size, protection, &mut old) }
        == 0
    {
        return Err(windows_error(label));
    }
    Ok(())
}

fn inject_payload(target: HANDLE, path: &Path) -> Result<()> {
    let payload = Payload::parse(read_payload(path)?)?;
    let image = allocate(target, payload.image_size, "VirtualAllocEx(image)")? as usize;
    let mapped = payload.map_at(image as u64)?;
    write_remote(
        target,
        image,
        &mapped[..payload.headers_size],
        "WriteProcessMemory(headers)",
    )?;
    for section in &payload.sections {
        let size = section.copy_size();
        if size != 0 {
            write_remote(
                target,
                image + section.rva,
                &mapped[section.rva..section.rva + size],
                "WriteProcessMemory(section)",
            )?;
        }
    }
    protect_remote(
        target,
        image,
        payload.headers_size,
        PAGE_READONLY,
        "VirtualProtectEx(headers)",
    )?;
    for section in &payload.sections {
        let size = section.memory_size();
        if size != 0 {
            protect_remote(
                target,
                image + section.rva,
                size,
                section.protection(),
                "VirtualProtectEx(section)",
            )?;
        }
    }
    if unsafe { FlushInstructionCache(target, image as *const c_void, payload.image_size) } == 0 {
        return Err(windows_error("FlushInstructionCache(payload)"));
    }
    let context = unsafe {
        platform::context::BootstrapContext::resolve(image as u64, |module, export| {
            GetProcAddress(
                GetModuleHandleA(module.as_ptr().cast()),
                export.as_ptr().cast(),
            )
        })
    }
    .ok_or("Cannot resolve payload bootstrap exports")?;
    let context_bytes = unsafe {
        std::slice::from_raw_parts(
            (&context as *const platform::context::BootstrapContext).cast::<u8>(),
            size_of_val(&context),
        )
    };
    let remote_context = allocate(target, context_bytes.len(), "VirtualAllocEx(context)")?;
    write_remote(
        target,
        remote_context as usize,
        context_bytes,
        "WriteProcessMemory(context)",
    )?;
    let entry = payload.entry_address(image);
    let thread = unsafe {
        CreateRemoteThread(
            target,
            null(),
            0,
            Some(std::mem::transmute::<
                usize,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(entry)),
            remote_context,
            0,
            null_mut(),
        )
    };
    if thread.is_null() {
        return Err(windows_error("CreateRemoteThread(payload)"));
    }
    let thread = unsafe { Handle::from_raw(thread) };
    let wait = unsafe { WaitForSingleObject(thread.raw(), 30000) };
    if wait != WAIT_OBJECT_0 {
        return Err("payload thread did not finish (exit 0x00000000)".into());
    }
    let mut exit_code = 0;
    if unsafe { GetExitCodeThread(thread.raw(), &mut exit_code) } == 0 || exit_code != 1 {
        return Err(format!("payload entry failed (exit 0x{exit_code:08X})"));
    }
    println!("manual-mapped payload at 0x{image:X}");
    Ok(())
}

#[cfg(feature = "testhost")]
fn inject_test_dll(target: HANDLE, path: &Path) -> Result<()> {
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFreeEx};
    let library = unsafe { GetModuleHandleA(c"kernel32.dll".as_ptr().cast()) };
    let load = unsafe { GetProcAddress(library, c"LoadLibraryW".as_ptr().cast()) }
        .ok_or_else(|| windows_error("GetProcAddress(LoadLibraryW)"))?;
    let path = wide(path.as_os_str())?;
    let bytes = unsafe {
        std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), path.len() * size_of::<u16>())
    };
    let remote = allocate(target, bytes.len(), "VirtualAllocEx(path)")?;
    write_remote(target, remote as usize, bytes, "WriteProcessMemory(path)")?;
    let thread = unsafe {
        CreateRemoteThread(
            target,
            null(),
            0,
            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(load)),
            remote,
            0,
            null_mut(),
        )
    };
    if thread.is_null() {
        return Err(windows_error("CreateRemoteThread(LoadLibraryW)"));
    }
    let thread = unsafe { Handle::from_raw(thread) };
    let wait = unsafe { WaitForSingleObject(thread.raw(), 30000) };
    let mut module = 0;
    let got_exit_code =
        wait == WAIT_OBJECT_0 && unsafe { GetExitCodeThread(thread.raw(), &mut module) } != 0;
    unsafe {
        VirtualFreeEx(target, remote, 0, MEM_RELEASE);
    }
    if !got_exit_code || module == 0 {
        return Err(if wait == WAIT_OBJECT_0 {
            "remote LoadLibraryW failed"
        } else {
            "remote thread did not finish"
        }
        .into());
    }
    Ok(())
}
