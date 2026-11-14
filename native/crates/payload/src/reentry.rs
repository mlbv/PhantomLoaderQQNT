// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::{managed, runtime};
use core::{
    arch::asm,
    ffi::{CStr, c_void},
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
};
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_ENOUGH_MEMORY, ERROR_PROC_NOT_FOUND, HANDLE, WAIT_OBJECT_0},
    Security::SECURITY_ATTRIBUTES,
};

type TlsAlloc = unsafe extern "system" fn() -> u32;
type TlsFree = unsafe extern "system" fn(u32) -> i32;
type CreateEvent =
    unsafe extern "system" fn(*const SECURITY_ATTRIBUTES, i32, i32, *const u16) -> HANDLE;
type SetEvent = unsafe extern "system" fn(HANDLE) -> i32;
type Wait = unsafe extern "system" fn(HANDLE, u32) -> u32;
type CreateThread = unsafe extern "system" fn(
    *const SECURITY_ATTRIBUTES,
    usize,
    Option<unsafe extern "system" fn(*mut c_void) -> u32>,
    *mut c_void,
    u32,
    *mut u32,
) -> HANDLE;

struct Api {
    event: HANDLE,
    set_event: SetEvent,
    wait: Wait,
    create_thread: CreateThread,
}

const UNINITIALIZED: u32 = u32::MAX;
const PRIMARY_SLOTS: u32 = 64;
const TLS_SLOTS_OFFSET: usize = 0x1480;

static INITIALIZE_LOCK: runtime::Lock = runtime::Lock::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static TLS_SLOT: AtomicU32 = AtomicU32::new(UNINITIALIZED);
static LEASE_SLOT: AtomicU32 = AtomicU32::new(UNINITIALIZED);
static API: AtomicPtr<Api> = AtomicPtr::new(ptr::null_mut());
static mut API_STORAGE: MaybeUninit<Api> = MaybeUninit::uninit();

fn api() -> &'static Api {
    unsafe { &*API.load(Ordering::Acquire) }
}

unsafe fn depth(slot: u32) -> usize {
    let value;
    unsafe {
        asm!(
            "mov {value}, gs:[{offset}]",
            value = out(reg) value,
            offset = in(reg) TLS_SLOTS_OFFSET + slot as usize * size_of::<usize>(),
            options(nostack, readonly, preserves_flags),
        );
    }
    value
}

unsafe fn set_depth(slot: u32, value: usize) {
    unsafe {
        asm!(
            "mov gs:[{offset}], {value}",
            offset = in(reg) TLS_SLOTS_OFFSET + slot as usize * size_of::<usize>(),
            value = in(reg) value,
            options(nostack, preserves_flags),
        );
    }
}

pub fn active() -> bool {
    let slot = TLS_SLOT.load(Ordering::Acquire);
    slot != UNINITIALIZED && unsafe { depth(slot) != 0 }
}

/// A stack-owned application execution on the current native thread.
#[repr(C)]
pub(crate) struct Lease {
    previous: *mut Lease,
    identity: *const c_void,
}

impl Lease {
    pub(crate) const fn new() -> Self {
        Self {
            previous: ptr::null_mut(),
            identity: ptr::null(),
        }
    }

    /// The node must stay at this address until `leave`, including during unwind.
    pub(crate) unsafe fn enter(&mut self, identity: *const c_void) {
        let slot = LEASE_SLOT.load(Ordering::Acquire);
        self.previous = unsafe { depth(slot) } as *mut Lease;
        self.identity = identity;
        unsafe { set_depth(slot, ptr::from_mut(self) as usize) };
    }

    pub(crate) unsafe fn leave(&self) {
        let slot = LEASE_SLOT.load(Ordering::Acquire);
        unsafe { set_depth(slot, self.previous as usize) };
    }
}

pub(crate) fn enclosing(identity: *const c_void) -> usize {
    let slot = LEASE_SLOT.load(Ordering::Acquire);
    let mut current = unsafe { depth(slot) } as *const Lease;
    let mut count = 0;
    while !current.is_null() {
        let lease = unsafe { &*current };
        count += usize::from(lease.identity == identity);
        current = lease.previous;
    }
    count
}

/// Suppresses managed callbacks on the current native thread, including API callees.
pub struct Suppression {
    slot: u32,
    previous: usize,
    _thread: PhantomData<*mut ()>,
}

impl Suppression {
    pub fn enter() -> Self {
        let slot = TLS_SLOT.load(Ordering::Acquire);
        let previous = if slot == UNINITIALIZED {
            0
        } else {
            unsafe {
                let previous = depth(slot);
                set_depth(slot, previous + 1);
                previous
            }
        };
        Self {
            slot,
            previous,
            _thread: PhantomData,
        }
    }
}

impl Drop for Suppression {
    fn drop(&mut self) {
        if self.slot != UNINITIALIZED {
            unsafe { set_depth(self.slot, self.previous) };
        }
    }
}

unsafe fn resolve<T: Copy>(
    module: windows_sys::Win32::Foundation::HMODULE,
    name: &CStr,
) -> Result<T, u32> {
    let address = unsafe { runtime::resolve(module, name) }.ok_or(ERROR_PROC_NOT_FOUND)?;
    Ok(unsafe { mem::transmute_copy(&address) })
}

/// Reserves primary TLS and starts the resident reclaimer before hook activation.
///
/// Published TLS and event state remain process-owned, including if thread
/// creation fails, because another thread can already hold a suppression guard.
pub fn initialize() -> Result<(), u32> {
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    let guard = INITIALIZE_LOCK.lock();
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    if API.load(Ordering::Acquire).is_null() {
        unsafe {
            let host = runtime::api();
            let name = [
                107u16, 101, 114, 110, 101, 108, 51, 50, 46, 100, 108, 108, 0,
            ];
            let module = (host.get_module_handle_w)(name.as_ptr());
            if module.is_null() {
                return Err((host.get_last_error)());
            }
            let allocate: TlsAlloc = resolve(module, c"TlsAlloc")?;
            let free: TlsFree = resolve(module, c"TlsFree")?;
            let create_event: CreateEvent = resolve(module, c"CreateEventW")?;
            let set_event = resolve(module, c"SetEvent")?;
            let wait = resolve(module, c"WaitForSingleObject")?;
            let create_thread = resolve(module, c"CreateThread")?;
            let slot = allocate();
            if slot == UNINITIALIZED {
                return Err(ERROR_NOT_ENOUGH_MEMORY);
            }
            if slot >= PRIMARY_SLOTS {
                if free(slot) == 0 {
                    runtime::fatal();
                }
                return Err(ERROR_NOT_ENOUGH_MEMORY);
            }
            let lease_slot = allocate();
            if lease_slot >= PRIMARY_SLOTS {
                if lease_slot != UNINITIALIZED && free(lease_slot) == 0 || free(slot) == 0 {
                    runtime::fatal();
                }
                return Err(ERROR_NOT_ENOUGH_MEMORY);
            }
            let event = create_event(ptr::null(), 0, 0, ptr::null());
            if event.is_null() {
                let error = (host.get_last_error)();
                if free(lease_slot) == 0 || free(slot) == 0 {
                    runtime::fatal();
                }
                return Err(error);
            }
            (&raw mut API_STORAGE).write(MaybeUninit::new(Api {
                event,
                set_event,
                wait,
                create_thread,
            }));
            LEASE_SLOT.store(lease_slot, Ordering::Release);
            TLS_SLOT.store(slot, Ordering::Release);
            API.store((&raw mut API_STORAGE).cast::<Api>(), Ordering::Release);
        }
    }
    let _suppression = Suppression::enter();
    let thread = unsafe {
        (api().create_thread)(
            ptr::null(),
            0,
            Some(reclaimer),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
        )
    };
    if thread.is_null() {
        let error = unsafe { (runtime::api().get_last_error)() };
        drop(guard);
        return Err(error);
    }
    if unsafe { (runtime::api().close_handle)(thread) } == 0 {
        runtime::fatal();
    }
    INITIALIZED.store(true, Ordering::Release);
    drop(guard);
    Ok(())
}

/// Wakes the resident reclaimer without taking a registry lock or allocating.
pub fn reclaim() {
    let _suppression = Suppression::enter();
    if unsafe { (api().set_event)(api().event) } == 0 {
        runtime::fatal();
    }
}

unsafe extern "system" fn reclaimer(_: *mut c_void) -> u32 {
    let _suppression = Suppression::enter();
    loop {
        if unsafe { (api().wait)(api().event, u32::MAX) } != WAIT_OBJECT_0 {
            runtime::fatal();
        }
        managed::reclaim_pending();
    }
}
