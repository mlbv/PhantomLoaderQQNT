// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::reentry;
use core::{
    alloc::{GlobalAlloc, Layout},
    cell::UnsafeCell,
    ffi::{CStr, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use platform::context::{BootstrapContext, Win32Api};
use windows_sys::Win32::{
    Foundation::{HANDLE, HMODULE},
    System::{
        Diagnostics::Debug::{
            IMAGE_DIRECTORY_ENTRY_EXCEPTION, IMAGE_NT_HEADERS64, IMAGE_RUNTIME_FUNCTION_ENTRY,
        },
        SystemServices::{FAST_FAIL_FATAL_APP_EXIT, IMAGE_DOS_HEADER, MEMORY_ALLOCATION_ALIGNMENT},
        Threading::SRWLOCK,
    },
};

static CONTEXT: AtomicPtr<BootstrapContext> = AtomicPtr::new(ptr::null_mut());
static HEAP: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

pub fn api() -> &'static Win32Api {
    unsafe { &*CONTEXT.load(Ordering::Acquire) }
}

pub fn heap() -> HANDLE {
    HEAP.load(Ordering::Relaxed)
}

pub unsafe fn image_headers(base: *const u8) -> &'static IMAGE_NT_HEADERS64 {
    unsafe {
        let dos = &*base.cast::<IMAGE_DOS_HEADER>();
        &*base.add(dos.e_lfanew as usize).cast::<IMAGE_NT_HEADERS64>()
    }
}

pub unsafe fn initialize(context: *mut BootstrapContext) -> bool {
    unsafe {
        if !CONTEXT.load(Ordering::Acquire).is_null() {
            return reentry::initialize().is_ok();
        }
        let host = &*context;
        let image = host.image_base as *const u8;
        let unwind = image_headers(image).OptionalHeader.DataDirectory
            [IMAGE_DIRECTORY_ENTRY_EXCEPTION as usize];
        if unwind.Size != 0
            && !(host.rtl_add_function_table)(
                image.add(unwind.VirtualAddress as usize).cast(),
                unwind.Size / size_of::<IMAGE_RUNTIME_FUNCTION_ENTRY>() as u32,
                host.image_base,
            )
        {
            return false;
        }
        let heap = (host.heap_create)(0, 0, 0);
        if heap.is_null() {
            return false;
        }
        HEAP.store(heap, Ordering::Relaxed);
        CONTEXT.store(context, Ordering::Release);
        reentry::initialize().is_ok()
    }
}

pub unsafe fn resolve(
    module: HMODULE,
    name: &CStr,
) -> Option<unsafe extern "system" fn() -> isize> {
    let _suppression = reentry::Suppression::enter();
    unsafe { (api().get_proc_address)(module, name.as_ptr()) }
}

pub fn fatal() -> ! {
    unsafe {
        core::arch::asm!("int 0x29", in("ecx") FAST_FAIL_FATAL_APP_EXIT, options(noreturn, nostack))
    }
}

pub struct Lock(UnsafeCell<SRWLOCK>);
unsafe impl Sync for Lock {}

impl Lock {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(SRWLOCK {
            Ptr: ptr::null_mut(),
        }))
    }

    pub fn lock(&self) -> Guard<'_> {
        let suppression = reentry::Suppression::enter();
        unsafe { (api().acquire_srw_lock_exclusive)(self.0.get()) };
        Guard {
            lock: self,
            _suppression: suppression,
        }
    }
}

pub struct Guard<'a> {
    lock: &'a Lock,
    _suppression: reentry::Suppression,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        let _suppression = reentry::Suppression::enter();
        unsafe { (api().release_srw_lock_exclusive)(self.lock.0.get()) };
    }
}

struct PrivateHeap;

#[global_allocator]
static ALLOCATOR: PrivateHeap = PrivateHeap;

unsafe impl GlobalAlloc for PrivateHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _suppression = reentry::Suppression::enter();
        unsafe {
            if layout.align() <= MEMORY_ALLOCATION_ALIGNMENT as usize {
                return (api().heap_alloc)(heap(), 0, layout.size()).cast();
            }
            let Some(bytes) = layout
                .size()
                .checked_add(layout.align() - 1 + size_of::<usize>())
            else {
                return ptr::null_mut();
            };
            let allocation = (api().heap_alloc)(heap(), 0, bytes).cast::<u8>();
            if allocation.is_null() {
                return allocation;
            }
            let aligned = ((allocation as usize + size_of::<usize>() + layout.align() - 1)
                & !(layout.align() - 1)) as *mut u8;
            aligned.cast::<*mut u8>().sub(1).write(allocation);
            aligned
        }
    }

    unsafe fn dealloc(&self, address: *mut u8, layout: Layout) {
        let _suppression = reentry::Suppression::enter();
        unsafe {
            let allocation = if layout.align() <= MEMORY_ALLOCATION_ALIGNMENT as usize {
                address
            } else {
                address.cast::<*mut u8>().sub(1).read()
            };
            if (api().heap_free)(heap(), 0, allocation.cast()) == 0 {
                fatal();
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    unsafe {
        let to = destination.cast::<u8>();
        let from = source.cast::<u8>();
        for index in 0..count {
            to.add(index)
                .write_volatile(from.add(index).read_volatile());
        }
    }
    destination
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    unsafe {
        let to = destination.cast::<u8>();
        let from = source.cast::<u8>();
        if (to as usize) <= from as usize {
            memcpy(destination, source, count);
        } else {
            for index in (0..count).rev() {
                to.add(index)
                    .write_volatile(from.add(index).read_volatile());
            }
        }
    }
    destination
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(destination: *mut c_void, value: i32, count: usize) -> *mut c_void {
    unsafe {
        let to = destination.cast::<u8>();
        for index in 0..count {
            to.add(index).write_volatile(value as u8);
        }
    }
    destination
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, count: usize) -> i32 {
    unsafe {
        let left = left.cast::<u8>();
        let right = right.cast::<u8>();
        for index in 0..count {
            let a = left.add(index).read_volatile();
            let b = right.add(index).read_volatile();
            if a != b {
                return i32::from(a) - i32::from(b);
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn strlen(value: *const core::ffi::c_char) -> usize {
    unsafe {
        let mut count = 0;
        while value.add(count).read_volatile() != 0 {
            count += 1;
        }
        count
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn wcslen(value: *const u16) -> usize {
    unsafe {
        let mut count = 0;
        while value.add(count).read_volatile() != 0 {
            count += 1;
        }
        count
    }
}
