// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use core::{
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};
use platform::profile::{VERIFIER, VERIFIER_RETURN_VALUE};
use windows_sys::Win32::{
    Foundation::{HANDLE, HMODULE, MAX_PATH},
    System::{
        Diagnostics::Debug::{IMAGE_DIRECTORY_ENTRY_IMPORT, IMAGE_NT_OPTIONAL_HDR64_MAGIC},
        Memory::{PAGE_EXECUTE_READWRITE, PAGE_READWRITE},
        SystemServices::{
            IMAGE_DOS_HEADER, IMAGE_DOS_SIGNATURE, IMAGE_NT_SIGNATURE, IMAGE_ORDINAL_FLAG64,
        },
    },
};

use crate::{formatting::log, native, runtime};

#[repr(C)]
struct ImportDescriptor {
    original_first_thunk: u32,
    time_date_stamp: u32,
    forwarder_chain: u32,
    name: u32,
    first_thunk: u32,
}

type LoadExW = unsafe extern "system" fn(*const u16, HANDLE, u32) -> HMODULE;
type LoadExA = unsafe extern "system" fn(*const c_char, HANDLE, u32) -> HMODULE;
type LoadW = unsafe extern "system" fn(*const u16) -> HMODULE;
type LoadA = unsafe extern "system" fn(*const c_char) -> HMODULE;

static VERIFIER_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static mut LOAD_EX_W: *mut c_void = ptr::null_mut();
static mut LOAD_EX_A: *mut c_void = ptr::null_mut();
static mut LOAD_W: *mut c_void = ptr::null_mut();
static mut LOAD_A: *mut c_void = ptr::null_mut();

unsafe fn patch_verifier(module: HMODULE) {
    unsafe {
        if VERIFIER_ATTEMPTED.swap(true, Ordering::AcqRel) {
            return;
        }
        let api = runtime::api();
        let target = module.cast::<u8>().add(VERIFIER.rva);
        if core::slice::from_raw_parts(target, VERIFIER.prologue.len()) != VERIFIER.prologue {
            log(format_args!("verifier: prologue mismatch at {target:p}"));
            return;
        }
        let value = VERIFIER_RETURN_VALUE.to_le_bytes();
        let patch = [0xB8, value[0], value[1], value[2], value[3], 0xC3];
        let mut previous = 0;
        if (api.virtual_protect)(
            target.cast(),
            patch.len(),
            PAGE_EXECUTE_READWRITE,
            &mut previous,
        ) == 0
        {
            log(format_args!(
                "verifier: VirtualProtect failed error={}",
                (api.get_last_error)()
            ));
            return;
        }
        ptr::copy_nonoverlapping(patch.as_ptr(), target, patch.len());
        let mut ignored = 0;
        if (api.virtual_protect)(target.cast(), patch.len(), previous, &mut ignored) == 0
            || (api.flush_instruction_cache)(
                (api.get_current_process)(),
                target.cast(),
                patch.len(),
            ) == 0
        {
            runtime::fatal();
        }
        log(format_args!("patched verifier at {target:p}"));
    }
}

unsafe fn module_loaded(module: HMODULE) {
    unsafe {
        let mut path = [0u16; MAX_PATH as usize];
        let length =
            (runtime::api().get_module_file_name_w)(module, path.as_mut_ptr(), path.len() as u32)
                as usize;
        if length == 0 || length >= path.len() {
            return;
        }
        let path = &path[..length];
        let start = path
            .iter()
            .rposition(|&c| c == b'\\' as u16 || c == b'/' as u16)
            .map_or(0, |index| index + 1);
        let name = &path[start..];
        if name.len() == b"QQNT.dll".len()
            && name
                .iter()
                .zip(b"QQNT.dll")
                .all(|(&wide, &byte)| wide <= 127 && (wide as u8).eq_ignore_ascii_case(&byte))
        {
            log(format_args!("QQNT.dll loaded at {module:p}"));
            patch_verifier(module);
            if native::register(module) {
                log(format_args!("registered phantomloader_native"));
            }
        }
    }
}

unsafe extern "system" fn load_ex_w(name: *const u16, file: HANDLE, flags: u32) -> HMODULE {
    unsafe {
        let function: LoadExW = core::mem::transmute(LOAD_EX_W);
        let module = function(name, file, flags);
        let last_error = (runtime::api().get_last_error)();
        if !module.is_null() {
            module_loaded(module);
        }
        (runtime::api().set_last_error)(last_error);
        module
    }
}

unsafe extern "system" fn load_ex_a(name: *const c_char, file: HANDLE, flags: u32) -> HMODULE {
    unsafe {
        let function: LoadExA = core::mem::transmute(LOAD_EX_A);
        let module = function(name, file, flags);
        let last_error = (runtime::api().get_last_error)();
        if !module.is_null() {
            module_loaded(module);
        }
        (runtime::api().set_last_error)(last_error);
        module
    }
}

unsafe extern "system" fn load_w(name: *const u16) -> HMODULE {
    unsafe {
        let function: LoadW = core::mem::transmute(LOAD_W);
        let module = function(name);
        let last_error = (runtime::api().get_last_error)();
        if !module.is_null() {
            module_loaded(module);
        }
        (runtime::api().set_last_error)(last_error);
        module
    }
}

unsafe extern "system" fn load_a(name: *const c_char) -> HMODULE {
    unsafe {
        let function: LoadA = core::mem::transmute(LOAD_A);
        let module = function(name);
        let last_error = (runtime::api().get_last_error)();
        if !module.is_null() {
            module_loaded(module);
        }
        (runtime::api().set_last_error)(last_error);
        module
    }
}

unsafe fn install_iat(
    module: HMODULE,
    name: &CStr,
    replacement: *mut c_void,
    original: *mut *mut c_void,
) -> bool {
    unsafe {
        let api = runtime::api();
        let base = module.cast::<u8>();
        if (*base.cast::<IMAGE_DOS_HEADER>()).e_magic != IMAGE_DOS_SIGNATURE {
            return false;
        }
        let nt = runtime::image_headers(base);
        if nt.Signature != IMAGE_NT_SIGNATURE
            || nt.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC
        {
            return false;
        }
        let directory = nt.OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT as usize];
        if directory.VirtualAddress == 0 {
            return false;
        }
        let mut descriptor = base
            .add(directory.VirtualAddress as usize)
            .cast::<ImportDescriptor>();
        while (*descriptor).name != 0 {
            let entry = &*descriptor;
            let dll = CStr::from_ptr(base.add(entry.name as usize).cast());
            if !dll.to_bytes().eq_ignore_ascii_case(b"KERNEL32.dll") {
                descriptor = descriptor.add(1);
                continue;
            }
            if entry.first_thunk == 0 {
                return false;
            }
            let names_rva = if entry.original_first_thunk == 0 {
                entry.first_thunk
            } else {
                entry.original_first_thunk
            };
            let mut names = base.add(names_rva as usize).cast::<u64>();
            let mut slot = base.add(entry.first_thunk as usize).cast::<*mut c_void>();
            while *names != 0 {
                let ordinal = *names;
                if ordinal & IMAGE_ORDINAL_FLAG64 == 0 {
                    let imported =
                        CStr::from_ptr(base.add(ordinal as usize + size_of::<u16>()).cast());
                    if imported.to_bytes().eq_ignore_ascii_case(name.to_bytes()) {
                        let mut previous = 0;
                        if (api.virtual_protect)(
                            slot.cast(),
                            size_of::<usize>(),
                            PAGE_READWRITE,
                            &mut previous,
                        ) == 0
                        {
                            return false;
                        }
                        original.write(*slot);
                        slot.write(replacement);
                        let mut ignored = 0;
                        if (api.virtual_protect)(
                            slot.cast(),
                            size_of::<usize>(),
                            previous,
                            &mut ignored,
                        ) == 0
                            || (api.flush_instruction_cache)(
                                (api.get_current_process)(),
                                slot.cast(),
                                size_of::<usize>(),
                            ) == 0
                        {
                            runtime::fatal();
                        }
                        return true;
                    }
                }
                names = names.add(1);
                slot = slot.add(1);
            }
            return false;
        }
        false
    }
}

pub unsafe fn install() {
    unsafe {
        let api = runtime::api();
        log(format_args!("payload mapped at {:#x}", api.image_base));
        let exe = (api.get_module_handle_w)(ptr::null());
        if exe.is_null() {
            log(format_args!("GetModuleHandleW(NULL) failed"));
            return;
        }
        for (name, replacement, original) in [
            (
                c"LoadLibraryExW",
                load_ex_w as *mut c_void,
                &raw mut LOAD_EX_W,
            ),
            (
                c"LoadLibraryExA",
                load_ex_a as *mut c_void,
                &raw mut LOAD_EX_A,
            ),
            (c"LoadLibraryW", load_w as *mut c_void, &raw mut LOAD_W),
            (c"LoadLibraryA", load_a as *mut c_void, &raw mut LOAD_A),
        ] {
            if install_iat(exe, name, replacement, original) {
                log(format_args!("hooked {}", name.to_str().unwrap()));
            } else {
                log(format_args!("not installed: {}", name.to_str().unwrap()));
            }
        }
    }
}
