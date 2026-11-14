// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::{managed, reentry, runtime};
use alloc::vec::Vec;
use core::{
    ffi::{CStr, c_void},
    mem::{self, MaybeUninit},
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, Ordering},
};
use platform::coordinator::{ENVIRONMENT_VARIABLE, MAGIC, Request, Response, VERSION};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_ENVVAR_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
        ERROR_NOT_ENOUGH_MEMORY, ERROR_PROC_NOT_FOUND, ERROR_WRITE_FAULT, GENERIC_READ,
        GENERIC_WRITE, HANDLE, HMODULE, INVALID_HANDLE_VALUE, STATUS_PENDING,
    },
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::OPEN_EXISTING,
    System::{
        Diagnostics::Debug::{
            EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
        },
        IO::IO_STATUS_BLOCK,
    },
};

type Handler = unsafe extern "system" fn(*mut EXCEPTION_POINTERS) -> i32;
type AddHandler = unsafe extern "system" fn(u32, Option<Handler>) -> *mut c_void;
type Environment = unsafe extern "system" fn(*const u16, *mut u16, u32) -> u32;
type CreateEvent =
    unsafe extern "system" fn(*const SECURITY_ATTRIBUTES, i32, i32, *const u16) -> HANDLE;
type Event = unsafe extern "system" fn(HANDLE) -> i32;
type NtWait = unsafe extern "system" fn(HANDLE, u8, *const i64) -> i32;
type IoApc = unsafe extern "system" fn(*mut c_void, *mut IO_STATUS_BLOCK, u32);
type NtIo = unsafe extern "system" fn(
    HANDLE,
    HANDLE,
    Option<IoApc>,
    *mut c_void,
    *mut IO_STATUS_BLOCK,
    *mut c_void,
    u32,
    *const i64,
    *const u32,
) -> i32;

struct Api {
    pipe: HANDLE,
    create_event: CreateEvent,
    reset_event: Event,
    set_event: Event,
    providers: [managed::Provider; 3],
}

const DORMANT: u8 = 0;
const ACTIVE: u8 = 1;
const COMPLETED: u8 = 2;
const BREAKPOINT: u32 = 0x8000_0003;

struct Gate {
    target: u64,
    event: HANDLE,
    next: *mut Gate,
    state: AtomicU8,
}

static INITIALIZE_LOCK: runtime::Lock = runtime::Lock::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static GATES: AtomicPtr<Gate> = AtomicPtr::new(ptr::null_mut());
static mut API: MaybeUninit<Api> = MaybeUninit::uninit();

fn api() -> &'static Api {
    unsafe { &*(&raw const API).cast::<Api>() }
}

pub(crate) fn providers() -> &'static [managed::Provider; 3] {
    &api().providers
}

unsafe fn resolve(
    module: HMODULE,
    name: &CStr,
) -> Result<unsafe extern "system" fn() -> isize, u32> {
    unsafe { runtime::resolve(module, name) }.ok_or(ERROR_PROC_NOT_FOUND)
}

unsafe fn connect(environment: Environment) -> Result<HANDLE, u32> {
    unsafe {
        let host = runtime::api();
        let mut name = [0u16; ENVIRONMENT_VARIABLE.len() + 1];
        for (destination, source) in name.iter_mut().zip(ENVIRONMENT_VARIABLE.bytes()) {
            *destination = u16::from(source);
        }
        (host.set_last_error)(0);
        let required = environment(name.as_ptr(), ptr::null_mut(), 0);
        if required == 0 {
            let error = (host.get_last_error)();
            return Err(if error == 0 {
                ERROR_ENVVAR_NOT_FOUND
            } else {
                error
            });
        }
        let mut path = Vec::new();
        path.try_reserve_exact(required as usize)
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        path.resize(required as usize, 0u16);
        (host.set_last_error)(0);
        let length = environment(name.as_ptr(), path.as_mut_ptr(), required);
        if length == 0 {
            let error = (host.get_last_error)();
            return Err(if error == 0 {
                ERROR_ENVVAR_NOT_FOUND
            } else {
                error
            });
        }
        if length >= required {
            return Err(ERROR_INSUFFICIENT_BUFFER);
        }
        let pipe = (host.create_file_w)(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        );
        if pipe == INVALID_HANDLE_VALUE {
            return Err((host.get_last_error)());
        }
        Ok(pipe)
    }
}

/// Requires the payload runtime to have been initialized. The pipe and VEH remain resident.
pub fn initialize() -> Result<(), u32> {
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    let _lock = INITIALIZE_LOCK.lock();
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    unsafe {
        let host = runtime::api();
        let mut name = [0u16; 13];
        for (destination, source) in name.iter_mut().zip(c"kernel32.dll".to_bytes()) {
            *destination = u16::from(*source);
        }
        let module = (host.get_module_handle_w)(name.as_ptr());
        if module.is_null() {
            return Err((host.get_last_error)());
        }
        let add_handler: AddHandler =
            mem::transmute(resolve(module, c"AddVectoredExceptionHandler")?);
        let environment: Environment = mem::transmute(resolve(module, c"GetEnvironmentVariableW")?);
        let mut native_name = [0u16; 10];
        for (destination, source) in native_name.iter_mut().zip(c"ntdll.dll".to_bytes()) {
            *destination = u16::from(*source);
        }
        let native_module = (host.get_module_handle_w)(native_name.as_ptr());
        if native_module.is_null() {
            return Err((host.get_last_error)());
        }
        let provider = |index, name| -> Result<managed::Provider, u32> {
            let address = resolve(native_module, name)? as usize;
            #[cfg(feature = "fixture-api")]
            let address = fixture::provider(index, address);
            #[cfg(not(feature = "fixture-api"))]
            let _ = index;
            managed::Provider::new(address)
        };
        let mut functions = Api {
            pipe: ptr::null_mut(),
            create_event: mem::transmute(resolve(module, c"CreateEventW")?),
            reset_event: mem::transmute(resolve(module, c"ResetEvent")?),
            set_event: mem::transmute(resolve(module, c"SetEvent")?),
            providers: [
                provider(0, c"NtWaitForSingleObject")?,
                provider(1, c"NtReadFile")?,
                provider(2, c"NtWriteFile")?,
            ],
        };
        functions.pipe = connect(environment)?;
        if add_handler(1, Some(exception_handler)).is_null() {
            let error = (host.get_last_error)();
            if (host.close_handle)(functions.pipe) == 0 {
                runtime::fatal();
            }
            return Err(error);
        }
        // No gate is published until the table is immutable and initialization is visible.
        (&raw mut API).write(MaybeUninit::new(functions));
        INITIALIZED.store(true, Ordering::Release);
    }
    Ok(())
}

unsafe fn gate(target: u64) -> Result<&'static Gate, u32> {
    unsafe {
        let head = GATES.load(Ordering::Acquire);
        let mut current = head;
        while !current.is_null() {
            let entry = &*current;
            if entry.target == target {
                return Ok(entry);
            }
            current = entry.next;
        }
        let host = runtime::api();
        let allocation = (host.heap_alloc)(runtime::heap(), 0, size_of::<Gate>()).cast::<Gate>();
        if allocation.is_null() {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        let event = (api().create_event)(ptr::null(), 1, 1, ptr::null());
        if event.is_null() {
            let error = (host.get_last_error)();
            if (host.heap_free)(runtime::heap(), 0, allocation.cast()) == 0 {
                runtime::fatal();
            }
            return Err(error);
        }
        allocation.write(Gate {
            target,
            event,
            next: head,
            state: AtomicU8::new(DORMANT),
        });
        // Nodes and their events are never reclaimed, including after detach.
        GATES.store(allocation, Ordering::Release);
        Ok(&*allocation)
    }
}

fn with_native_wait<T>(call: impl FnOnce(NtWait, Option<&managed::InternalLease>) -> T) -> T {
    let provider = &providers()[0];
    provider.with_original(|original| {
        let address = original.map_or(provider.address(), |lease| lease.address());
        call(unsafe { mem::transmute(address) }, original)
    })
}

fn native_wait(handle: HANDLE) -> i32 {
    with_native_wait(|wait, _| unsafe { wait(handle, 0, ptr::null()) })
}

fn gate_wait(handle: HANDLE) -> i32 {
    with_native_wait(|wait, original| {
        #[cfg(feature = "fixture-api")]
        fixture::veh_lease(wait, original);
        #[cfg(not(feature = "fixture-api"))]
        let _ = original;
        unsafe { wait(handle, 0, ptr::null()) }
    })
}

unsafe fn transfer(data: *mut u8, length: u32, write: bool) -> Result<usize, u32> {
    unsafe {
        let provider = &providers()[if write { 2 } else { 1 }];
        provider.with_original(|original| {
            let address = original.map_or(provider.address(), |lease| lease.address());
            let provider: NtIo = mem::transmute(address);
            #[cfg(feature = "fixture-api")]
            if !write {
                fixture::read_lease(original);
            }
            let mut completion: IO_STATUS_BLOCK = mem::zeroed();
            let mut status = provider(
                api().pipe,
                ptr::null_mut(),
                None,
                ptr::null_mut(),
                &mut completion,
                data.cast(),
                length,
                ptr::null(),
                ptr::null(),
            );
            if status == STATUS_PENDING {
                // Keep the provider lease, completion block and caller buffer alive until completion.
                if native_wait(api().pipe) != 0 {
                    runtime::fatal();
                }
                status = completion.Anonymous.Status;
            }
            if status < 0 {
                Err(ERROR_BROKEN_PIPE)
            } else {
                Ok(completion.Information)
            }
        })
    }
}

unsafe fn write_exact(mut data: *const u8, mut remaining: usize) -> Result<(), u32> {
    unsafe {
        while remaining != 0 {
            let written = transfer(
                data.cast_mut(),
                remaining.min(u32::MAX as usize) as u32,
                true,
            )?;
            if written == 0 {
                return Err(ERROR_WRITE_FAULT);
            }
            data = data.add(written);
            remaining -= written;
        }
        Ok(())
    }
}

unsafe fn read_exact(mut data: *mut u8, mut remaining: usize) -> Result<(), u32> {
    #[cfg(feature = "fixture-api")]
    fixture::before_read();
    unsafe {
        while remaining != 0 {
            let received = transfer(data, remaining.min(u32::MAX as usize) as u32, false)?;
            if received == 0 {
                return Err(ERROR_BROKEN_PIPE);
            }
            data = data.add(received);
            remaining -= received;
        }
        Ok(())
    }
}

/// # Safety
/// The caller must serialize all changes and gate creation through the hooks registry lock.
/// The target must remain live, with intrinsic INT3 entry bytes forbidden, for the process lifetime.
/// The requesting thread must not execute the target while its rendezvous is closed.
/// `Ok` means the patch is committed. On `Err`, the after bytes never admitted
/// an entry, and any modified bytes, peer contexts and protections were restored.
pub unsafe fn change(request: &Request) -> Result<(), u32> {
    let _suppression = reentry::Suppression::enter();
    initialize()?;
    unsafe {
        let entry = gate(request.target)?;
        if (api().reset_event)(entry.event) == 0 {
            return Err((runtime::api().get_last_error)());
        }
        entry.state.store(ACTIVE, Ordering::Release);
        if write_exact(ptr::from_ref(request).cast(), size_of::<Request>()).is_err() {
            runtime::fatal();
        }
        let mut response = MaybeUninit::<Response>::uninit();
        if read_exact(response.as_mut_ptr().cast(), size_of::<Response>()).is_err() {
            runtime::fatal();
        }
        let response = response.assume_init();
        if response.magic != MAGIC || response.version != VERSION || response.reserved != 0 {
            runtime::fatal();
        }
        // A validated error also means the broker has completed any rollback and resumed threads.
        entry.state.store(COMPLETED, Ordering::Release);
        if (api().set_event)(entry.event) == 0 {
            runtime::fatal();
        }
        #[cfg(feature = "fixture-api")]
        fixture::responded();
        if response.error == 0 {
            Ok(())
        } else {
            Err(response.error)
        }
    }
}

#[cfg(feature = "fixture-api")]
mod fixture {
    use super::*;
    static BEFORE_READ: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static READ_RELEASE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static READ_LEASED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static VEH_LEASED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static VEH_RELEASE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static RESPONDED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
    static PROVIDERS: [AtomicPtr<c_void>; 3] = [const { AtomicPtr::new(ptr::null_mut()) }; 3];

    #[unsafe(no_mangle)]
    pub extern "C" fn FixtureCoordinatorProviders(
        wait: *mut c_void,
        read: *mut c_void,
        write: *mut c_void,
    ) {
        for (slot, address) in PROVIDERS.iter().zip([wait, read, write]) {
            slot.store(address, Ordering::Release);
        }
    }

    pub fn provider(index: usize, address: usize) -> usize {
        let replacement = PROVIDERS[index].load(Ordering::Acquire);
        if replacement.is_null() {
            address
        } else {
            replacement as usize
        }
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn FixtureCoordinatorEvents(
        before_read: HANDLE,
        read_release: HANDLE,
        read_leased: HANDLE,
        veh_leased: HANDLE,
        veh_release: HANDLE,
        responded: HANDLE,
    ) {
        READ_RELEASE.store(read_release, Ordering::Relaxed);
        VEH_RELEASE.store(veh_release, Ordering::Relaxed);
        READ_LEASED.store(read_leased, Ordering::Release);
        VEH_LEASED.store(veh_leased, Ordering::Release);
        RESPONDED.store(responded, Ordering::Release);
        BEFORE_READ.store(before_read, Ordering::Release);
    }

    fn signal(event: HANDLE) {
        if unsafe { (api().set_event)(event) } == 0 {
            runtime::fatal();
        }
    }

    pub fn before_read() {
        let event = BEFORE_READ.swap(ptr::null_mut(), Ordering::AcqRel);
        if !event.is_null() {
            signal(event);
            if native_wait(READ_RELEASE.load(Ordering::Acquire)) != 0 {
                runtime::fatal();
            }
        }
    }

    pub fn read_lease(original: Option<&managed::InternalLease>) {
        let event = READ_LEASED.swap(ptr::null_mut(), Ordering::AcqRel);
        if !event.is_null() {
            if original.is_none() {
                runtime::fatal();
            }
            signal(event);
        }
    }

    pub fn veh_lease(wait: NtWait, original: Option<&managed::InternalLease>) {
        let event = VEH_LEASED.swap(ptr::null_mut(), Ordering::AcqRel);
        if !event.is_null() {
            if original.is_none() {
                runtime::fatal();
            }
            signal(event);
            let release = VEH_RELEASE.load(Ordering::Acquire);
            if !release.is_null() && unsafe { wait(release, 0, ptr::null()) } != 0 {
                runtime::fatal();
            }
        }
    }

    pub fn responded() {
        let event = RESPONDED.swap(ptr::null_mut(), Ordering::AcqRel);
        if !event.is_null() {
            signal(event);
        }
    }
}

unsafe extern "system" fn exception_handler(exception: *mut EXCEPTION_POINTERS) -> i32 {
    let _suppression = reentry::Suppression::enter();
    unsafe {
        let record = &*(*exception).ExceptionRecord;
        if record.ExceptionCode as u32 != BREAKPOINT {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let address = record.ExceptionAddress as usize as u64;
        let mut current = GATES.load(Ordering::Acquire);
        while !current.is_null() {
            let entry = &*current;
            if entry.target == address && entry.state.load(Ordering::Acquire) != DORMANT {
                if gate_wait(entry.event) != 0 {
                    runtime::fatal();
                }
                (*(*exception).ContextRecord).Rip = entry.target;
                return EXCEPTION_CONTINUE_EXECUTION;
            }
            current = entry.next;
        }
        EXCEPTION_CONTINUE_SEARCH
    }
}
