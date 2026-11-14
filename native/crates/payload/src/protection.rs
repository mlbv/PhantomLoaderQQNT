// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::{coordinator, hooks, reentry, runtime};
use alloc::boxed::Box;
use core::{
    ffi::{CStr, c_void},
    mem, ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use neohook::Platform;
use platform::profile::{INSPECTION_PROFILE, InspectionProfile};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, ERROR_PROC_NOT_FOUND, HANDLE, HMODULE},
    System::{
        Diagnostics::Debug::{IMAGE_NT_HEADERS64, IMAGE_NT_OPTIONAL_HDR64_MAGIC},
        Memory::{
            MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ,
            PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY,
            PAGE_READWRITE, PAGE_WRITECOPY,
        },
        SystemInformation::IMAGE_FILE_MACHINE_AMD64,
        SystemServices::{IMAGE_DOS_HEADER, IMAGE_DOS_SIGNATURE, IMAGE_NT_SIGNATURE},
    },
};

type MapView = unsafe extern "system" fn(
    HANDLE,
    HANDLE,
    *mut *mut c_void,
    usize,
    usize,
    *mut i64,
    *mut usize,
    u32,
    u32,
    u32,
) -> i32;
type MapViewEx = unsafe extern "system" fn(
    HANDLE,
    HANDLE,
    *mut *mut c_void,
    *mut i64,
    *mut usize,
    u32,
    u32,
    *mut c_void,
    u32,
) -> i32;
type CompareObjectHandles = unsafe extern "system" fn(HANDLE, HANDLE) -> i32;
type QueryVirtualMemory = unsafe extern "system" fn(
    HANDLE,
    *const c_void,
    u32,
    *mut ImageInformation,
    usize,
    *mut usize,
) -> i32;

#[repr(C)]
struct ImageInformation {
    base: *mut c_void,
    size: usize,
    flags: u32,
}

const MEMORY_IMAGE_INFORMATION: u32 = 6;
const IMAGE_NOT_EXECUTABLE: u32 = 1 << 1;

struct Api {
    canonical: [*const u8; 2],
    compare_object_handles: CompareObjectHandles,
    query_virtual_memory: QueryVirtualMemory,
    profile: InspectionProfile,
}

static INSTALL_LOCK: runtime::Lock = runtime::Lock::new();
static API: AtomicPtr<Api> = AtomicPtr::new(ptr::null_mut());
static mut OWNER: u8 = 0;
static mut MAP: *mut c_void = ptr::null_mut();
static mut MAP_EX: *mut c_void = ptr::null_mut();

#[cfg(feature = "fixture-api")]
static MATCHES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn api() -> &'static Api {
    unsafe { &*API.load(Ordering::Acquire) }
}

pub(crate) fn attachment_target(canonical: *const u8) -> *const u8 {
    let api = API.load(Ordering::Acquire);
    if api.is_null() {
        return canonical;
    }
    let targets = unsafe { &(*api).canonical };
    if canonical == targets[0] {
        map_view as *const u8
    } else if canonical == targets[1] {
        map_view_ex as *const u8
    } else {
        canonical
    }
}

unsafe fn resolve(module: HMODULE, name: &CStr) -> Result<*const u8, u32> {
    let export = unsafe { runtime::resolve(module, name) }.ok_or(ERROR_PROC_NOT_FOUND)?;
    unsafe {
        neohook::resolve::<hooks::Backend>(export as *const u8, |address| {
            hooks::reject_generated(address)?;
            Ok(hooks::active_target(address))
        })
    }
}

unsafe fn install(profile: InspectionProfile) -> Result<(), u32> {
    let _lock = INSTALL_LOCK.lock();
    if !API.load(Ordering::Acquire).is_null() {
        return Err(ERROR_ALREADY_EXISTS);
    }
    coordinator::initialize()?;
    unsafe {
        let host = runtime::api();
        let kernel = (host.get_module_handle_w)(windows_sys::w!("kernelbase.dll"));
        let native = (host.get_module_handle_w)(windows_sys::w!("ntdll.dll"));
        let compare_object_handles =
            runtime::resolve(kernel, c"CompareObjectHandles").ok_or(ERROR_PROC_NOT_FOUND)?;
        let functions = Box::new(Api {
            canonical: [
                resolve(native, c"NtMapViewOfSection")?,
                resolve(native, c"NtMapViewOfSectionEx")?,
            ],
            compare_object_handles: mem::transmute::<
                unsafe extern "system" fn() -> isize,
                CompareObjectHandles,
            >(compare_object_handles),
            query_virtual_memory: mem::transmute::<
                unsafe extern "system" fn() -> isize,
                QueryVirtualMemory,
            >(
                runtime::resolve(native, c"NtQueryVirtualMemory").ok_or(ERROR_PROC_NOT_FOUND)?,
            ),
            profile,
        });
        let targets = functions.canonical;
        API.store(Box::into_raw(functions), Ordering::Release);
        hooks::attach_slot(
            (&raw mut OWNER).cast(),
            targets[0].cast_mut().cast(),
            map_view as *mut c_void,
            &raw mut MAP,
        )?;
        hooks::attach_slot(
            (&raw mut OWNER).cast(),
            targets[1].cast_mut().cast(),
            map_view_ex as *mut c_void,
            &raw mut MAP_EX,
        )?;
    }
    Ok(())
}

pub(crate) unsafe fn initialize() -> Result<(), u32> {
    unsafe { install(INSPECTION_PROFILE) }
}

unsafe fn region(base: *const u8, address: *const u8) -> Option<MEMORY_BASIC_INFORMATION> {
    let region = unsafe { <hooks::Backend as Platform>::query(address) }.ok()?;
    (region.Type == MEM_IMAGE && region.AllocationBase == base.cast_mut().cast()).then_some(region)
}

unsafe fn readable(base: *const u8, image_size: usize, offset: usize, length: usize) -> bool {
    let Some(end) = offset.checked_add(length).filter(|&end| end <= image_size) else {
        return false;
    };
    let mut offset = offset;
    while offset < end {
        let address = unsafe { base.add(offset) };
        let Some(region) = (unsafe { region(base, address) }) else {
            return false;
        };
        const READABLE: u32 = PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY;
        if region.State != MEM_COMMIT
            || region.Protect & PAGE_GUARD != 0
            || region.Protect & READABLE == 0
        {
            return false;
        }
        offset += (region.RegionSize - (address as usize - region.BaseAddress as usize))
            .min(end - offset);
    }
    true
}

unsafe fn observe(process: HANDLE, output: *mut *mut c_void, view_size: *mut usize) {
    unsafe {
        let functions = api();
        if process != -1isize as HANDLE
            && (functions.compare_object_handles)(process, -1isize as HANDLE) == 0
        {
            return;
        }
        let base = (*output).cast::<u8>();
        let bytes = *view_size;
        if !readable(base, bytes, 0, size_of::<IMAGE_DOS_HEADER>()) {
            return;
        }
        let dos = ptr::read_unaligned(base.cast::<IMAGE_DOS_HEADER>());
        if dos.e_magic != IMAGE_DOS_SIGNATURE || dos.e_lfanew < size_of::<IMAGE_DOS_HEADER>() as i32
        {
            return;
        }
        let nt_offset = dos.e_lfanew as usize;
        if !readable(base, bytes, nt_offset, size_of::<IMAGE_NT_HEADERS64>()) {
            return;
        }
        let nt = ptr::read_unaligned(base.add(nt_offset).cast::<IMAGE_NT_HEADERS64>());
        let profile = functions.profile;
        if nt.Signature != IMAGE_NT_SIGNATURE
            || nt.FileHeader.Machine != IMAGE_FILE_MACHINE_AMD64
            || nt.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC
            || nt.FileHeader.TimeDateStamp != profile.time_date_stamp
            || nt.OptionalHeader.SizeOfImage != profile.image_size
        {
            return;
        }
        let mut image = mem::zeroed();
        if (functions.query_virtual_memory)(
            -1isize as HANDLE,
            base.cast(),
            MEMORY_IMAGE_INFORMATION,
            &mut image,
            size_of::<ImageInformation>(),
            ptr::null_mut(),
        ) < 0
        {
            runtime::fatal();
        }
        if image.flags & IMAGE_NOT_EXECUTABLE != 0 {
            return;
        }
        let image_size = (profile.image_size as usize).min(bytes);
        for patch in profile.patches {
            if !readable(base, image_size, patch.rva, patch.expected.len())
                || core::slice::from_raw_parts(base.add(patch.rva), patch.expected.len())
                    != patch.expected
            {
                runtime::fatal();
            }
        }
        let host = runtime::api();
        for patch in profile.patches {
            let mut offset = 0;
            while offset < patch.replacement.len() {
                let target = base.add(patch.rva + offset);
                let region = region(base, target).unwrap_or_else(|| runtime::fatal());
                let length = (region.RegionSize - (target as usize - region.BaseAddress as usize))
                    .min(patch.replacement.len() - offset);
                let mut previous = 0;
                if (host.virtual_protect)(
                    target.cast(),
                    length,
                    PAGE_EXECUTE_READWRITE,
                    &mut previous,
                ) == 0
                {
                    runtime::fatal();
                }
                ptr::copy_nonoverlapping(patch.replacement.as_ptr().add(offset), target, length);
                let mut ignored = 0;
                if (host.virtual_protect)(target.cast(), length, previous, &mut ignored) == 0 {
                    runtime::fatal();
                }
                offset += length;
            }
            if (host.flush_instruction_cache)(
                -1isize as HANDLE,
                base.add(patch.rva).cast(),
                patch.replacement.len(),
            ) == 0
            {
                runtime::fatal();
            }
        }
        #[cfg(feature = "fixture-api")]
        MATCHES.fetch_add(1, Ordering::Relaxed);
    }
}

#[inline(never)]
unsafe extern "system" fn map_view(
    section: HANDLE,
    process: HANDLE,
    base: *mut *mut c_void,
    zero_bits: usize,
    commit_size: usize,
    section_offset: *mut i64,
    view_size: *mut usize,
    inherit: u32,
    allocation: u32,
    protection: u32,
) -> i32 {
    unsafe {
        let original: MapView = mem::transmute(MAP);
        let status = original(
            section,
            process,
            base,
            zero_bits,
            commit_size,
            section_offset,
            view_size,
            inherit,
            allocation,
            protection,
        );
        let _suppression = reentry::Suppression::enter();
        let host = runtime::api();
        let last_error = (host.get_last_error)();
        if status >= 0 {
            observe(process, base, view_size);
        }
        (host.set_last_error)(last_error);
        status
    }
}

#[inline(never)]
unsafe extern "system" fn map_view_ex(
    section: HANDLE,
    process: HANDLE,
    base: *mut *mut c_void,
    section_offset: *mut i64,
    view_size: *mut usize,
    allocation: u32,
    protection: u32,
    parameters: *mut c_void,
    parameter_count: u32,
) -> i32 {
    unsafe {
        let original: MapViewEx = mem::transmute(MAP_EX);
        let status = original(
            section,
            process,
            base,
            section_offset,
            view_size,
            allocation,
            protection,
            parameters,
            parameter_count,
        );
        let _suppression = reentry::Suppression::enter();
        let host = runtime::api();
        let last_error = (host.get_last_error)();
        if status >= 0 {
            observe(process, base, view_size);
        }
        (host.set_last_error)(last_error);
        status
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn FixtureArmInspection(
    timestamp: u32,
    image_size: u32,
    rva: usize,
    expected: *const u8,
    replacement: *const u8,
    length: usize,
) -> u32 {
    use platform::profile::ImagePatch;
    use windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;

    if expected.is_null()
        || replacement.is_null()
        || length == 0
        || rva
            .checked_add(length)
            .is_none_or(|end| end > image_size as usize)
    {
        return ERROR_INVALID_PARAMETER;
    }
    unsafe {
        let patches = Box::leak(Box::new([ImagePatch {
            rva,
            expected: Box::leak(
                core::slice::from_raw_parts(expected, length)
                    .to_vec()
                    .into_boxed_slice(),
            ),
            replacement: Box::leak(
                core::slice::from_raw_parts(replacement, length)
                    .to_vec()
                    .into_boxed_slice(),
            ),
        }]));
        install(InspectionProfile {
            time_date_stamp: timestamp,
            image_size,
            patches,
        })
        .err()
        .unwrap_or(0)
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureInspectionMatches() -> usize {
    MATCHES.load(Ordering::Relaxed)
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn FixtureArmProductionInspection() -> u32 {
    unsafe { install(INSPECTION_PROFILE) }.err().unwrap_or(0)
}
