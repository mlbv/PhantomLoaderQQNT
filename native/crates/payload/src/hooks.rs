// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::{coordinator, runtime};
use alloc::vec::Vec;
use core::{
    cell::UnsafeCell,
    ffi::{CStr, c_void},
    mem, ptr,
};
use neohook::{InlineHook, Platform};
use platform::coordinator::{ATTACH, DETACH, Redirect, Request};
use windows_sys::Win32::{
    Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_FOUND,
        ERROR_PROC_NOT_FOUND,
    },
    System::{
        Diagnostics::Debug::IMAGE_RUNTIME_FUNCTION_ENTRY,
        Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE,
        },
        SystemInformation::SYSTEM_INFO,
    },
};

pub struct Attachment {
    pub id: u64,
}

pub(crate) struct AttachFailure {
    pub error: u32,
    pub hook: Option<InlineHook<Backend>>,
}

impl From<u32> for AttachFailure {
    fn from(error: u32) -> Self {
        Self { error, hook: None }
    }
}

impl AttachFailure {
    fn release_quiescent(self) -> u32 {
        if let Some(hook) = self.hook {
            unsafe { hook.release() };
        }
        self.error
    }
}

struct Record {
    owner: *mut c_void,
    id: u64,
    requested: *const u8,
    hook: InlineHook<Backend>,
    slot: *mut *mut c_void,
    redirect: Option<Redirect>,
}

struct Registry {
    records: Vec<Record>,
    next_id: u64,
}

struct Shared(UnsafeCell<Registry>);
unsafe impl Sync for Shared {}

static LOCK: runtime::Lock = runtime::Lock::new();
static REGISTRY: Shared = Shared(UnsafeCell::new(Registry {
    records: Vec::new(),
    next_id: 1,
}));

impl Registry {
    fn active_target(&self, address: *const u8) -> bool {
        self.records
            .iter()
            .any(|record| record.hook.target() == address)
    }

    unsafe fn reject_generated(&self, address: *const u8) -> Result<(), u32> {
        if self.records.is_empty() {
            return Ok(());
        }
        let region = unsafe { Backend::query(address)? };
        if self
            .records
            .iter()
            .any(|record| record.hook.allocation().cast::<c_void>() == region.AllocationBase)
        {
            return Err(ERROR_ALREADY_EXISTS);
        }
        Ok(())
    }

    fn overlaps(&self, hook: &InlineHook<Backend>) -> bool {
        self.records.iter().any(|record| record.hook.overlaps(hook))
    }
}

pub(crate) fn reject_generated(address: *const u8) -> Result<(), u32> {
    let _lock = LOCK.lock();
    let registry = unsafe { &*REGISTRY.0.get() };
    unsafe { registry.reject_generated(address) }
}

pub(crate) fn active_target(address: *const u8) -> bool {
    let _lock = LOCK.lock();
    let registry = unsafe { &*REGISTRY.0.get() };
    registry.active_target(address)
}

/// Publishes the prepared trampoline before activation. The callback runs under
/// the hooks registry lock and must not reenter the registry. A failed activation
/// transfers the hook to the caller, which must close admission and drain leases.
pub fn attach_managed(
    owner: *mut c_void,
    target: *mut c_void,
    id: u64,
    dispatcher: *mut c_void,
    publish: impl FnOnce(*mut c_void),
) -> Result<Attachment, AttachFailure> {
    unsafe {
        attach_inner(
            owner,
            target,
            dispatcher,
            ptr::null_mut(),
            Some(id),
            publish,
        )
    }
}

/// The slot and referenced code must remain resident for the process lifetime.
/// Replacement and original callers must be quiescent before rollback or fixture disposal.
pub unsafe fn attach_slot(
    owner: *mut c_void,
    target: *mut c_void,
    replacement: *mut c_void,
    slot: *mut *mut c_void,
) -> Result<Attachment, u32> {
    if slot.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    unsafe { attach_inner(owner, target, replacement, slot, None, |_| {}) }
        .map_err(AttachFailure::release_quiescent)
}

unsafe fn attach_inner(
    owner: *mut c_void,
    target: *mut c_void,
    replacement: *mut c_void,
    slot: *mut *mut c_void,
    managed: Option<u64>,
    publish: impl FnOnce(*mut c_void),
) -> Result<Attachment, AttachFailure> {
    let _lock = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    let target = target.cast::<u8>();
    let replacement = replacement.cast::<u8>();
    let canonical = unsafe {
        neohook::resolve::<Backend>(target, |address| {
            registry.reject_generated(address)?;
            Ok(registry.active_target(address))
        })?
    };
    if registry
        .records
        .iter()
        .any(|record| record.requested == target || record.hook.target() == canonical)
    {
        return Err(ERROR_ALREADY_EXISTS.into());
    }
    let resolved_replacement = unsafe {
        neohook::resolve::<Backend>(replacement, |address| {
            registry.reject_generated(address)?;
            Ok(registry.active_target(address))
        })?
    };
    if canonical == resolved_replacement {
        return Err(ERROR_INVALID_PARAMETER.into());
    }
    let hook = unsafe {
        match managed {
            Some(id) => InlineHook::<Backend>::prepare_managed(canonical, id, replacement)?,
            None => InlineHook::<Backend>::prepare(canonical, replacement)?,
        }
    };
    if managed.is_some()
        && let Err(error) = hook.validate_managed_activation()
    {
        unsafe { hook.release() };
        return Err(error.into());
    }
    let redirect = hook.gateway_range().map(|(start, end)| Redirect {
        start: start as u64,
        end: end as u64,
        destination: replacement as u64,
        token: managed.unwrap(),
    });
    if registry.overlaps(&hook) {
        unsafe { hook.release() };
        return Err(ERROR_ALREADY_EXISTS.into());
    }
    if registry.records.try_reserve(1).is_err() {
        unsafe { hook.release() };
        return Err(ERROR_NOT_ENOUGH_MEMORY.into());
    }
    let id = registry.next_id;
    let original = hook.original().cast();
    publish(original);
    if let Err(error) = unsafe { change(&hook, true, slot, original, redirect) } {
        return Err(AttachFailure {
            error,
            hook: Some(hook),
        });
    }
    registry.next_id += 1;
    registry.records.push(Record {
        owner,
        id,
        requested: target,
        hook,
        slot,
        redirect,
    });
    Ok(Attachment { id })
}

/// Unpatches the target while transferring generated-code ownership to the caller.
/// The caller must retain the hook until managed activations have drained.
pub fn detach_retained(owner: *mut c_void, id: u64) -> Result<InlineHook<Backend>, u32> {
    let _lock = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    let index = registry
        .records
        .iter()
        .position(|record| record.id == id && record.owner == owner)
        .ok_or(ERROR_NOT_FOUND)?;
    unsafe { remove_record(registry, index) }
}

/// The caller must establish quiescence for every hook owned by this owner.
#[cfg(feature = "fixture-api")]
pub fn dispose(owner: *mut c_void) -> Result<(), u32> {
    let _lock = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    while let Some(index) = registry
        .records
        .iter()
        .position(|record| record.owner == owner)
    {
        let hook = unsafe { remove_record(registry, index)? };
        unsafe { hook.release() };
    }
    Ok(())
}

unsafe fn remove_record(registry: &mut Registry, index: usize) -> Result<InlineHook<Backend>, u32> {
    let record = &registry.records[index];
    unsafe {
        change(
            &record.hook,
            false,
            record.slot,
            record.requested.cast_mut().cast(),
            record.redirect,
        )?
    };
    Ok(registry.records.remove(index).hook)
}

unsafe fn change(
    hook: &InlineHook<Backend>,
    attaching: bool,
    slot: *mut *mut c_void,
    published: *mut c_void,
    redirect: Option<Redirect>,
) -> Result<(), u32> {
    let (operation, before, after) = if attaching {
        (ATTACH, hook.original_bytes(), hook.patched_bytes())
    } else {
        (DETACH, hook.patched_bytes(), hook.original_bytes())
    };
    let mut request = Request::new(operation, hook.target() as u64, before, after);
    if !attaching && let Some(redirect) = redirect {
        request.redirects[0] = redirect;
        request.redirect_count = 1;
    }
    let previous = if attaching && !slot.is_null() {
        let previous = unsafe { slot.read() };
        unsafe { slot.write(published) };
        previous
    } else {
        ptr::null_mut()
    };
    if let Err(error) = unsafe { coordinator::change(&request) } {
        if attaching && !slot.is_null() {
            unsafe { slot.write(previous) };
        }
        return Err(error);
    }
    if !attaching && !slot.is_null() {
        unsafe { slot.write(published) };
    }
    Ok(())
}

pub(crate) struct Backend;

unsafe impl Platform for Backend {
    unsafe fn query(address: *const u8) -> Result<MEMORY_BASIC_INFORMATION, u32> {
        let mut region = unsafe { mem::zeroed() };
        if unsafe {
            (runtime::api().virtual_query)(
                address.cast(),
                &mut region,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return Err(unsafe { (runtime::api().get_last_error)() });
        }
        Ok(region)
    }

    unsafe fn allocate(address: *const u8, size: usize) -> *mut u8 {
        unsafe {
            (runtime::api().virtual_alloc)(
                address.cast(),
                size,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_EXECUTE_READWRITE,
            )
            .cast()
        }
    }

    unsafe fn free(address: *mut u8) {
        if unsafe { (runtime::api().virtual_free)(address.cast(), 0, MEM_RELEASE) } == 0 {
            runtime::fatal();
        }
    }

    unsafe fn protect(address: *const u8, size: usize, protection: u32) -> Result<u32, u32> {
        let mut old = 0;
        if unsafe { (runtime::api().virtual_protect)(address.cast(), size, protection, &mut old) }
            == 0
        {
            return Err(unsafe { (runtime::api().get_last_error)() });
        }
        Ok(old)
    }

    unsafe fn flush(address: *const u8, size: usize) -> Result<(), u32> {
        let api = runtime::api();
        if unsafe {
            (api.flush_instruction_cache)((api.get_current_process)(), address.cast(), size)
        } == 0
        {
            return Err(unsafe { (api.get_last_error)() });
        }
        Ok(())
    }

    fn system_info() -> Result<SYSTEM_INFO, u32> {
        type Function = unsafe extern "system" fn(*mut SYSTEM_INFO);
        let function: Function =
            unsafe { mem::transmute(resolve(c"kernel32.dll", c"GetSystemInfo")?) };
        let mut info = unsafe { mem::zeroed() };
        unsafe { function(&mut info) };
        Ok(info)
    }

    unsafe fn lookup_function(
        address: u64,
        base: &mut u64,
    ) -> Result<*const IMAGE_RUNTIME_FUNCTION_ENTRY, u32> {
        type Function = unsafe extern "system" fn(
            u64,
            *mut u64,
            *mut c_void,
        ) -> *const IMAGE_RUNTIME_FUNCTION_ENTRY;
        let function: Function =
            unsafe { mem::transmute(resolve(c"ntdll.dll", c"RtlLookupFunctionEntry")?) };
        Ok(unsafe { function(address, base, ptr::null_mut()) })
    }

    unsafe fn add_function(
        entry: *const IMAGE_RUNTIME_FUNCTION_ENTRY,
        base: u64,
    ) -> Result<(), u32> {
        if !unsafe { (runtime::api().rtl_add_function_table)(entry, 1, base) } {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        Ok(())
    }

    unsafe fn delete_function(entry: *const IMAGE_RUNTIME_FUNCTION_ENTRY) {
        type Function = unsafe extern "system" fn(*const IMAGE_RUNTIME_FUNCTION_ENTRY) -> bool;
        let function: Function = match unsafe { resolve(c"ntdll.dll", c"RtlDeleteFunctionTable") } {
            Ok(address) => unsafe { mem::transmute(address) },
            Err(_) => runtime::fatal(),
        };
        if !unsafe { function(entry) } {
            runtime::fatal();
        }
    }

    fn fatal() -> ! {
        runtime::fatal()
    }
}

unsafe fn resolve(module: &CStr, name: &CStr) -> Result<unsafe extern "system" fn() -> isize, u32> {
    let api = runtime::api();
    let mut wide = [0u16; 32];
    for (destination, source) in wide.iter_mut().zip(module.to_bytes()) {
        *destination = *source as u16;
    }
    let module = unsafe { (api.get_module_handle_w)(wide.as_ptr()) };
    if module.is_null() {
        return Err(unsafe { (api.get_last_error)() });
    }
    unsafe { (api.get_proc_address)(module, name.as_ptr()) }.ok_or(ERROR_PROC_NOT_FOUND)
}
