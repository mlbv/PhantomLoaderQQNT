// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::{
    foreign,
    formatting::Text,
    hooks, reentry,
    runtime::{self, Lock},
};
use core::{
    ffi::c_void,
    fmt::Write,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use platform::{
    incident::{ARM_FAILED, CAPTURED, CaptureState, DUMP_FAILED, MAX_CAPTURE_DIRECTORY_UNITS},
    profile::{
        APPEND_TASK, EXECUTE_PROGRAM, GROW_TASKS, QQ_VERSION, WRAPPER_IMAGE_SIZE, WRAPPER_SHA256,
        WRAPPER_TIMESTAMP,
    },
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_BAD_EXE_FORMAT, ERROR_FILENAME_EXCED_RANGE,
        ERROR_PROC_NOT_FOUND, ERROR_WRITE_FAULT, FILETIME, GENERIC_WRITE, HANDLE, HMODULE,
        INVALID_HANDLE_VALUE, MAX_PATH,
    },
    Storage::FileSystem::{
        CREATE_ALWAYS, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ,
    },
    System::SystemInformation::IMAGE_FILE_MACHINE_AMD64,
};

const U64_DECIMAL_BYTES: usize = u64::MAX.ilog10() as usize + 1;
const U32_DECIMAL_BYTES: usize = u32::MAX.ilog10() as usize + 1;
const STEM_BYTES: usize = "dlvm-".len() + U64_DECIMAL_BYTES + 2 * ("-".len() + U32_DECIMAL_BYTES);
const FILENAME_BYTES: usize = STEM_BYTES + ".json".len();
const STATUS_BYTES: usize =
    "{\"state\":\"\",\"build\":\"\",\"wrapper_sha256\":\"\",\"win32_error\":}\n".len()
        + "waiting-for-wrapper".len()
        + QQ_VERSION.len()
        + WRAPPER_SHA256.len()
        + U32_DECIMAL_BYTES;
const METADATA_BYTES: usize =
    "{\"build\":\"\",\"wrapper_sha256\":\"\",\"stage\":\"\",\"task_type\":,\"received_bytes\":,\"captured_filetime\":\"\",\"payload_file\":\"\",\"payload_write_error\":}\n".len()
    + QQ_VERSION.len() + WRAPPER_SHA256.len() + "receive-grow".len()
    + 2 * U32_DECIMAL_BYTES + 2 * U64_DECIMAL_BYTES + FILENAME_BYTES;

type Append = unsafe extern "system" fn(*mut c_void, *const u8) -> u64;
type LdrLoad = unsafe extern "system" fn(*mut u16, *mut u32, *mut c_void, *mut HMODULE) -> i32;

struct CaptureApi {
    flush: unsafe extern "system" fn(HANDLE) -> i32,
    terminate: unsafe extern "system" fn(HANDLE, u32) -> i32,
    time: unsafe extern "system" fn(*mut FILETIME),
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static STATE: AtomicU32 = AtomicU32::new(CaptureState::Uninitialized as u32);
static INSTALL_LOCK: Lock = Lock::new();
static CAPTURE_LOCK: Lock = Lock::new();
static mut DIRECTORY: [u16; MAX_PATH as usize] = [0; MAX_PATH as usize];
static mut DIRECTORY_LENGTH: usize = 0;
static mut CAPTURE_API: MaybeUninit<CaptureApi> = MaybeUninit::uninit();
static mut OWNER: u8 = 0;
static mut APPEND: *mut c_void = ptr::null_mut();
static mut GROW: *mut c_void = ptr::null_mut();
static mut EXECUTE: *mut c_void = ptr::null_mut();
static mut LOAD: *mut c_void = ptr::null_mut();

#[repr(C)]
pub struct Targets {
    pub append: *mut c_void,
    pub grow: *mut c_void,
    pub execute: *mut c_void,
}

fn capture_api() -> &'static CaptureApi {
    unsafe { &*(&raw const CAPTURE_API).cast::<CaptureApi>() }
}

pub fn status() -> u32 {
    STATE.load(Ordering::Acquire)
}

unsafe fn path(name: &str) -> [u16; MAX_PATH as usize] {
    unsafe {
        let mut result = [0u16; MAX_PATH as usize];
        let length = DIRECTORY_LENGTH;
        ptr::copy_nonoverlapping(
            (&raw const DIRECTORY).cast::<u16>(),
            result.as_mut_ptr(),
            length,
        );
        result[length] = b'\\' as u16;
        for (index, byte) in name.bytes().enumerate() {
            result[length + 1 + index] = u16::from(byte);
        }
        result
    }
}

unsafe fn write_and_flush(path: &[u16], data: *const u8, bytes: usize, disposition: u32) -> u32 {
    unsafe {
        let api = runtime::api();
        let file = (api.create_file_w)(
            path.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            ptr::null(),
            disposition,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_WRITE_THROUGH,
            ptr::null_mut(),
        );
        if file == INVALID_HANDLE_VALUE {
            return (api.get_last_error)();
        }
        let mut error = 0;
        let mut remaining = bytes;
        let mut current = data;
        while remaining != 0 {
            let mut written = 0;
            if (api.write_file)(
                file,
                current.cast(),
                remaining.min(u32::MAX as usize) as u32,
                &mut written,
                ptr::null_mut(),
            ) == 0
            {
                error = (api.get_last_error)();
                break;
            }
            if written == 0 {
                error = ERROR_WRITE_FAULT;
                break;
            }
            current = current.add(written as usize);
            remaining -= written as usize;
        }
        if error == 0 && (capture_api().flush)(file) == 0 {
            error = (api.get_last_error)();
        }
        if (api.close_handle)(file) == 0 && error == 0 {
            error = (api.get_last_error)();
        }
        error
    }
}

unsafe fn write_status(state: &str, error: u32) -> u32 {
    unsafe {
        let mut json = Text::<STATUS_BYTES>::new();
        let _ = writeln!(
            json,
            "{{\"state\":\"{state}\",\"build\":\"{QQ_VERSION}\",\"wrapper_sha256\":\"{WRAPPER_SHA256}\",\"win32_error\":{error}}}"
        );
        write_and_flush(
            &path("guard-status.json"),
            json.as_bytes().as_ptr(),
            json.as_bytes().len(),
            CREATE_ALWAYS,
        )
    }
}

unsafe fn exit(code: u32) -> ! {
    unsafe {
        (capture_api().terminate)((runtime::api().get_current_process)(), code);
        runtime::fatal()
    }
}

unsafe fn capture(kind: u32, program: *const c_void, bytes: usize, stage: &str) -> ! {
    unsafe {
        // Hold until process termination so another intercepted call cannot execute a program.
        let _guard = CAPTURE_LOCK.lock();
        let api = runtime::api();
        let mut time = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        (capture_api().time)(&mut time);
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        let mut stem = Text::<STEM_BYTES>::new();
        let _ = write!(
            stem,
            "dlvm-{ticks}-{}-{}",
            (api.get_current_process_id)(),
            (api.get_current_thread_id)()
        );
        let mut filename = Text::<FILENAME_BYTES>::new();
        let _ = write!(filename, "{}.bin", stem.as_str());
        let error = write_and_flush(&path(filename.as_str()), program.cast(), bytes, CREATE_NEW);
        let mut json = Text::<METADATA_BYTES>::new();
        let _ = writeln!(
            json,
            "{{\"build\":\"{QQ_VERSION}\",\"wrapper_sha256\":\"{WRAPPER_SHA256}\",\"stage\":\"{stage}\",\"task_type\":{kind},\"received_bytes\":{bytes},\"captured_filetime\":\"{ticks}\",\"payload_file\":\"{}\",\"payload_write_error\":{error}}}",
            filename.as_str()
        );
        let mut sidecar = Text::<FILENAME_BYTES>::new();
        let _ = write!(sidecar, "{}.json", stem.as_str());
        let metadata_error = write_and_flush(
            &path(sidecar.as_str()),
            json.as_bytes().as_ptr(),
            json.as_bytes().len(),
            CREATE_NEW,
        );
        exit(if error != 0 || metadata_error != 0 {
            DUMP_FAILED
        } else {
            CAPTURED
        });
    }
}

unsafe fn inspect(task: *const u8, stage: &str) {
    unsafe {
        if let Some(program) = foreign::task_program(task) {
            capture(program.kind, program.data, program.bytes, stage);
        }
    }
}

unsafe extern "system" fn append_task(vector: *mut c_void, task: *const u8) -> u64 {
    unsafe {
        inspect(task, "receive");
        let function: Append = core::mem::transmute(APPEND);
        function(vector, task)
    }
}

unsafe extern "system" fn grow_tasks(vector: *mut c_void, task: *const u8) -> u64 {
    unsafe {
        inspect(task, "receive-grow");
        let function: Append = core::mem::transmute(GROW);
        function(vector, task)
    }
}

unsafe extern "system" fn execute(program: *const c_void, bytes: usize) -> u64 {
    unsafe { capture(0, program, bytes, "execute") }
}

unsafe fn install(targets: &Targets) -> u32 {
    unsafe {
        let owner = (&raw mut OWNER).cast();
        let operation = (|| -> Result<(), u32> {
            hooks::attach_slot(
                owner,
                targets.execute,
                execute as *mut c_void,
                &raw mut EXECUTE,
            )?;
            hooks::attach_slot(
                owner,
                targets.append,
                append_task as *mut c_void,
                &raw mut APPEND,
            )?;
            hooks::attach_slot(
                owner,
                targets.grow,
                grow_tasks as *mut c_void,
                &raw mut GROW,
            )?;
            let error = write_status("armed", 0);
            if error != 0 {
                return Err(error);
            }
            Ok(())
        })();
        let error = operation.err().unwrap_or(0);
        STATE.store(
            if error == 0 {
                CaptureState::Armed
            } else {
                CaptureState::Failed
            } as u32,
            Ordering::Release,
        );
        error
    }
}

unsafe fn protect_wrapper(module: HMODULE) -> u32 {
    unsafe {
        let _guard = INSTALL_LOCK.lock();
        if status() == CaptureState::Armed as u32 {
            return 0;
        }
        let base = module.cast::<u8>();
        let nt = runtime::image_headers(base);
        let matches = nt.FileHeader.Machine == IMAGE_FILE_MACHINE_AMD64
            && nt.FileHeader.TimeDateStamp == WRAPPER_TIMESTAMP
            && nt.OptionalHeader.SizeOfImage == WRAPPER_IMAGE_SIZE
            && [APPEND_TASK, GROW_TASKS, EXECUTE_PROGRAM]
                .iter()
                .all(|site| {
                    core::slice::from_raw_parts(base.add(site.rva), site.prologue.len())
                        == site.prologue
                });
        let error = if matches {
            install(&Targets {
                append: base.add(APPEND_TASK.rva).cast(),
                grow: base.add(GROW_TASKS.rva).cast(),
                execute: base.add(EXECUTE_PROGRAM.rva).cast(),
            })
        } else {
            ERROR_BAD_EXE_FORMAT
        };
        if error != 0 {
            STATE.store(CaptureState::Failed as u32, Ordering::Release);
            write_status("failed", error);
        }
        error
    }
}

unsafe extern "system" fn load(
    search: *mut u16,
    flags: *mut u32,
    name: *mut c_void,
    module: *mut HMODULE,
) -> i32 {
    unsafe {
        let function: LdrLoad = core::mem::transmute(LOAD);
        let result = function(search, flags, name, module);
        let _suppression = reentry::Suppression::enter();
        let api = runtime::api();
        let last_error = (api.get_last_error)();
        if result >= 0 && (api.get_module_handle_w)(windows_sys::w!("wrapper.node")) == *module {
            if protect_wrapper(*module) != 0 {
                exit(ARM_FAILED);
            }
        }
        (api.set_last_error)(last_error);
        result
    }
}

pub unsafe fn initialize(directory: &[u16]) -> u32 {
    unsafe {
        if INITIALIZED.swap(true, Ordering::AcqRel) {
            return ERROR_ALREADY_EXISTS;
        }
        if directory.is_empty() || directory.len() > MAX_CAPTURE_DIRECTORY_UNITS {
            return ERROR_FILENAME_EXCED_RANGE;
        }
        ptr::copy_nonoverlapping(
            directory.as_ptr(),
            (&raw mut DIRECTORY).cast(),
            directory.len(),
        );
        DIRECTORY_LENGTH = directory.len();
        let kernel = (runtime::api().get_module_handle_w)(windows_sys::w!("kernel32.dll"));
        macro_rules! resolve {
            ($name:literal, $signature:ty) => {
                match runtime::resolve(kernel, $name) {
                    Some(function) => core::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        $signature,
                    >(function),
                    None => return ERROR_PROC_NOT_FOUND,
                }
            };
        }
        (&raw mut CAPTURE_API).write(MaybeUninit::new(CaptureApi {
            flush: resolve!(
                c"FlushFileBuffers",
                unsafe extern "system" fn(HANDLE) -> i32
            ),
            terminate: resolve!(
                c"TerminateProcess",
                unsafe extern "system" fn(HANDLE, u32) -> i32
            ),
            time: resolve!(
                c"GetSystemTimeAsFileTime",
                unsafe extern "system" fn(*mut FILETIME)
            ),
        }));
        let error = write_status("waiting-for-wrapper", 0);
        STATE.store(
            if error == 0 {
                CaptureState::WaitingForWrapper
            } else {
                CaptureState::Failed
            } as u32,
            Ordering::Release,
        );
        error
    }
}

pub unsafe fn arm(directory: &[u16]) -> u32 {
    unsafe {
        let error = initialize(directory);
        if error != 0 {
            return error;
        }
        let api = runtime::api();
        let ntdll = (api.get_module_handle_w)(windows_sys::w!("ntdll.dll"));
        let result = match runtime::resolve(ntdll, c"LdrLoadDll") {
            Some(function) => hooks::attach_slot(
                (&raw mut OWNER).cast(),
                function as *mut c_void,
                load as *mut c_void,
                &raw mut LOAD,
            ),
            None => Err(ERROR_PROC_NOT_FOUND),
        };
        let mut error = result.err().unwrap_or(0);
        if error == 0 {
            let module = (api.get_module_handle_w)(windows_sys::w!("wrapper.node"));
            if !module.is_null() {
                error = protect_wrapper(module);
            }
        }
        if error != 0 {
            STATE.store(CaptureState::Failed as u32, Ordering::Release);
            write_status("failed", error);
        }
        error
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ArmDlVmCapture(directory: *const u16) -> u32 {
    unsafe {
        let mut length = 0;
        while *directory.add(length) != 0 {
            length += 1;
        }
        arm(core::slice::from_raw_parts(directory, length))
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn InitializeDlVmCapture(directory: *const u16) -> u32 {
    unsafe {
        let mut length = 0;
        while *directory.add(length) != 0 {
            length += 1;
        }
        initialize(core::slice::from_raw_parts(directory, length))
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn InstallDlVmCaptureTargets(targets: *const Targets) -> u32 {
    unsafe {
        let _guard = INSTALL_LOCK.lock();
        install(&*targets)
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn DlVmCaptureStatus() -> u32 {
    status()
}
