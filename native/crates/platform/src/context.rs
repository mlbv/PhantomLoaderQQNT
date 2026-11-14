// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use core::ffi::{CStr, c_char, c_void};
use windows_sys::{
    Win32::{
        Foundation::{FARPROC, HANDLE, HMODULE},
        Security::SECURITY_ATTRIBUTES,
        System::{
            Diagnostics::{
                Debug::{CONTEXT, IMAGE_RUNTIME_FUNCTION_ENTRY},
                ToolHelp::THREADENTRY32,
            },
            Memory::MEMORY_BASIC_INFORMATION,
            Threading::SRWLOCK,
        },
    },
    core::BOOL,
};

pub const PAYLOAD_MAGIC: u32 = u32::from_le_bytes(*b"PHLD");
pub const PAYLOAD_ABI_VERSION: u32 = 4;

macro_rules! host_api {
    ($( $field:ident : $module:literal, $export:literal, $signature:ty; )+) => {
        /// Bootstrap ABI shared with the injector. Functions are resolved DLL exports,
        /// never addresses of import thunks in the injector's image.
        #[repr(C)]
        #[derive(Clone, Copy)]
        pub struct BootstrapContext {
            pub magic: u32,
            pub version: u32,
            pub image_base: u64,
            $(pub $field: $signature,)+
        }

        impl BootstrapContext {
            /// The resolver must return the actual exported function for each name.
            pub unsafe fn resolve(
                image_base: u64,
                mut resolver: impl FnMut(&CStr, &CStr) -> FARPROC,
            ) -> Option<Self> {
                Some(Self {
                    magic: PAYLOAD_MAGIC,
                    version: PAYLOAD_ABI_VERSION,
                    image_base,
                    $($field: unsafe {
                        core::mem::transmute::<unsafe extern "system" fn() -> isize, $signature>(
                            resolver(
                                CStr::from_bytes_with_nul_unchecked(concat!($module, "\0").as_bytes()),
                                CStr::from_bytes_with_nul_unchecked(concat!($export, "\0").as_bytes()),
                            )?,
                        )
                    },)+
                })
            }
        }
    }
}

host_api! {
    get_module_handle_w: "kernel32.dll", "GetModuleHandleW", unsafe extern "system" fn(*const u16) -> HMODULE;
    virtual_protect: "kernel32.dll", "VirtualProtect", unsafe extern "system" fn(*const c_void, usize, u32, *mut u32) -> BOOL;
    flush_instruction_cache: "kernel32.dll", "FlushInstructionCache", unsafe extern "system" fn(HANDLE, *const c_void, usize) -> BOOL;
    get_module_file_name_w: "kernel32.dll", "GetModuleFileNameW", unsafe extern "system" fn(HMODULE, *mut u16, u32) -> u32;
    get_current_process: "kernel32.dll", "GetCurrentProcess", unsafe extern "system" fn() -> HANDLE;
    get_current_process_id: "kernel32.dll", "GetCurrentProcessId", unsafe extern "system" fn() -> u32;
    create_file_w: "kernel32.dll", "CreateFileW", unsafe extern "system" fn(*const u16, u32, u32, *const SECURITY_ATTRIBUTES, u32, u32, HANDLE) -> HANDLE;
    write_file: "kernel32.dll", "WriteFile", unsafe extern "system" fn(HANDLE, *const c_void, u32, *mut u32, *mut c_void) -> BOOL;
    close_handle: "kernel32.dll", "CloseHandle", unsafe extern "system" fn(HANDLE) -> BOOL;
    get_temp_path_w: "kernel32.dll", "GetTempPathW", unsafe extern "system" fn(u32, *mut u16) -> u32;
    get_last_error: "kernel32.dll", "GetLastError", unsafe extern "system" fn() -> u32;
    get_proc_address: "kernel32.dll", "GetProcAddress", unsafe extern "system" fn(HMODULE, *const c_char) -> FARPROC;
    set_last_error: "kernel32.dll", "SetLastError", unsafe extern "system" fn(u32);
    virtual_query: "kernel32.dll", "VirtualQuery", unsafe extern "system" fn(*const c_void, *mut MEMORY_BASIC_INFORMATION, usize) -> usize;
    virtual_alloc: "kernel32.dll", "VirtualAlloc", unsafe extern "system" fn(*const c_void, usize, u32, u32) -> *mut c_void;
    virtual_free: "kernel32.dll", "VirtualFree", unsafe extern "system" fn(*mut c_void, usize, u32) -> BOOL;
    get_current_thread_id: "kernel32.dll", "GetCurrentThreadId", unsafe extern "system" fn() -> u32;
    get_current_thread: "kernel32.dll", "GetCurrentThread", unsafe extern "system" fn() -> HANDLE;
    suspend_thread: "kernel32.dll", "SuspendThread", unsafe extern "system" fn(HANDLE) -> u32;
    resume_thread: "kernel32.dll", "ResumeThread", unsafe extern "system" fn(HANDLE) -> u32;
    get_thread_context: "kernel32.dll", "GetThreadContext", unsafe extern "system" fn(HANDLE, *mut CONTEXT) -> BOOL;
    set_thread_context: "kernel32.dll", "SetThreadContext", unsafe extern "system" fn(HANDLE, *const CONTEXT) -> BOOL;
    heap_create: "kernel32.dll", "HeapCreate", unsafe extern "system" fn(u32, usize, usize) -> HANDLE;
    heap_alloc: "kernel32.dll", "HeapAlloc", unsafe extern "system" fn(HANDLE, u32, usize) -> *mut c_void;
    heap_free: "kernel32.dll", "HeapFree", unsafe extern "system" fn(HANDLE, u32, *const c_void) -> BOOL;
    create_toolhelp32_snapshot: "kernel32.dll", "CreateToolhelp32Snapshot", unsafe extern "system" fn(u32, u32) -> HANDLE;
    thread32_first: "kernel32.dll", "Thread32First", unsafe extern "system" fn(HANDLE, *mut THREADENTRY32) -> BOOL;
    thread32_next: "kernel32.dll", "Thread32Next", unsafe extern "system" fn(HANDLE, *mut THREADENTRY32) -> BOOL;
    open_thread: "kernel32.dll", "OpenThread", unsafe extern "system" fn(u32, BOOL, u32) -> HANDLE;
    acquire_srw_lock_exclusive: "kernel32.dll", "AcquireSRWLockExclusive", unsafe extern "system" fn(*mut SRWLOCK);
    release_srw_lock_exclusive: "kernel32.dll", "ReleaseSRWLockExclusive", unsafe extern "system" fn(*mut SRWLOCK);
    rtl_add_function_table: "kernel32.dll", "RtlAddFunctionTable", unsafe extern "system" fn(*const IMAGE_RUNTIME_FUNCTION_ENTRY, u32, u64) -> bool;
}

pub type Win32Api = BootstrapContext;
