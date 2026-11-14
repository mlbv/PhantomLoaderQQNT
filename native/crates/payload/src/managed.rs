// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::{coordinator, dispatch::Frame, hooks, protection, reentry, runtime};
use alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    cell::UnsafeCell,
    ffi::c_void,
    mem, ptr,
    sync::atomic::{AtomicPtr, AtomicUsize, Ordering},
};
use neohook::{InlineHook, Platform};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_INVALID_PARAMETER, ERROR_NOT_FOUND, HANDLE, WAIT_OBJECT_0,
};

type CreateEvent = unsafe extern "system" fn(*const c_void, i32, i32, *const u16) -> HANDLE;
type SetEvent = unsafe extern "system" fn(HANDLE) -> i32;
type WaitEvent = unsafe extern "system" fn(HANDLE, u32) -> u32;

unsafe fn kernel<T: Copy>(name: &core::ffi::CStr) -> T {
    let module = unsafe {
        (runtime::api().get_module_handle_w)(
            [
                107u16, 101, 114, 110, 101, 108, 51, 50, 46, 100, 108, 108, 0,
            ]
            .as_ptr(),
        )
    };
    let address = unsafe { runtime::resolve(module, name) }.unwrap_or_else(|| runtime::fatal());
    unsafe { mem::transmute_copy(&address) }
}

struct Event(HANDLE);

impl Event {
    fn new() -> Result<Self, u32> {
        let create: CreateEvent = unsafe { kernel(c"CreateEventW") };
        let handle = unsafe { create(ptr::null(), 1, 0, ptr::null()) };
        if handle.is_null() {
            return Err(unsafe { (runtime::api().get_last_error)() });
        }
        Ok(Self(handle))
    }

    fn signal(&self) {
        let set: SetEvent = unsafe { kernel(c"SetEvent") };
        if unsafe { set(self.0) } == 0 {
            runtime::fatal();
        }
    }

    fn wait(&self) {
        let wait: WaitEvent = unsafe { kernel(c"WaitForSingleObject") };
        if unsafe { wait(self.0, u32::MAX) } != WAIT_OBJECT_0 {
            runtime::fatal();
        }
    }
}

unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Drop for Event {
    fn drop(&mut self) {
        if unsafe { (runtime::api().close_handle)(self.0) } == 0 {
            runtime::fatal();
        }
    }
}

const OPEN: usize = 1;
const REFERENCE: usize = 2;

// A caller can reach resident dispatch after the ingress allocation was reclaimed.
struct Route {
    target: usize,
    stack_bytes: u64,
    callback: usize,
    user_data: u64,
    original: AtomicUsize,
    admission: AtomicUsize,
    record: AtomicPtr<Record>,
}

impl Route {
    fn references(&self) -> usize {
        self.admission.load(Ordering::Acquire) / REFERENCE
    }

    fn retain(&self) {
        self.admission.fetch_add(REFERENCE, Ordering::AcqRel);
    }

    fn admit(&self) -> bool {
        let mut state = self.admission.load(Ordering::Acquire);
        while state & OPEN != 0 {
            match self.admission.compare_exchange_weak(
                state,
                state + REFERENCE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => state = current,
            }
        }
        false
    }

    fn release(&self) -> usize {
        self.admission.fetch_sub(REFERENCE, Ordering::AcqRel) - REFERENCE
    }

    fn close(&self) {
        self.admission.fetch_and(!OPEN, Ordering::AcqRel);
        for provider in coordinator::providers() {
            let _ = provider.active.compare_exchange(
                ptr::from_ref(self).cast_mut(),
                ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

pub(crate) struct InternalLease {
    descriptor: *const Route,
    address: usize,
}

impl InternalLease {
    pub(crate) fn address(&self) -> usize {
        self.address
    }
}

impl Drop for InternalLease {
    fn drop(&mut self) {
        if route(self.descriptor).release() & OPEN == 0 {
            reentry::reclaim();
        }
    }
}

/// A native provider resolved before any managed entry can be changed.
pub(crate) struct Provider {
    export: usize,
    canonical: usize,
    active: AtomicPtr<Route>,
}

impl Provider {
    pub(crate) fn new(export: usize) -> Result<Self, u32> {
        let canonical =
            unsafe { neohook::resolve::<hooks::Backend>(export as *const u8, |_| Ok(false))? }
                as usize;
        Ok(Self {
            export,
            canonical,
            active: AtomicPtr::new(ptr::null_mut()),
        })
    }

    pub(crate) fn address(&self) -> usize {
        self.export
    }

    pub(crate) fn with_original<T>(&self, call: impl FnOnce(Option<&InternalLease>) -> T) -> T {
        let pointer = self.active.load(Ordering::Acquire);
        if pointer.is_null() {
            return call(None);
        }
        let descriptor = route(pointer);
        if !descriptor.admit() {
            return call(None);
        }
        let lease = InternalLease {
            descriptor: pointer,
            address: descriptor.original.load(Ordering::Acquire),
        };
        call(Some(&lease))
    }
}

fn route(pointer: *const Route) -> &'static Route {
    unsafe { &*pointer }
}

struct CleanupWait {
    enclosing: usize,
    event: Arc<Event>,
}

struct Record {
    owner: Option<*mut c_void>,
    backend_id: u64,
    route: *const Route,
    open: bool,
    retained: Option<InlineHook<hooks::Backend>>,
    drained: Option<Arc<Event>>,
    cleanup: Option<CleanupWait>,
    application: usize,
}

impl Record {
    fn retain_application(&mut self) {
        route(self.route).retain();
        self.application += 1;
    }

    fn release_application(&mut self) {
        self.application -= 1;
        route(self.route).release();
        self.reclaim();
    }

    fn is_drained(&self) -> bool {
        !self.open && self.retained.is_none()
    }

    fn reclaim(&mut self) {
        let descriptor = route(self.route);
        let references = descriptor.references();
        if self
            .cleanup
            .as_ref()
            .is_some_and(|wait| self.application <= wait.enclosing)
        {
            self.cleanup.take().unwrap().event.signal();
        }
        if !self.open && self.retained.is_some() && references == 0 {
            unsafe { self.retained.take().unwrap().release() };
            descriptor.original.store(0, Ordering::Release);
            self.drained.take().unwrap().signal();
        }
    }

    fn close(&mut self) -> Result<(), u32> {
        if self.open {
            let hook = hooks::detach_retained(self.owner.unwrap(), self.backend_id)?;
            self.open = false;
            self.retained = Some(hook);
            route(self.route).close();
            self.reclaim();
            if !self.is_drained() {
                // The reclaimer can hold this wait provider's lease while parked.
                reentry::reclaim();
            }
        }
        Ok(())
    }
}

enum TokenKind {
    Invocation(u32),
    Original,
}

struct Token {
    id: u64,
    record: u64,
    kind: TokenKind,
}

struct Registry {
    records: BTreeMap<u64, Box<Record>>,
    tokens: Vec<Token>,
    next_token: u64,
}

impl Registry {
    fn retire(&mut self, id: u64) {
        let record = self.records.remove(&id).unwrap();
        route(record.route)
            .record
            .store(ptr::null_mut(), Ordering::Release);
    }

    fn reclaim(&mut self, id: u64) {
        let record = self.records.get_mut(&id).unwrap();
        record.reclaim();
        if record.owner.is_none() && record.is_drained() {
            self.retire(id);
        }
    }

    fn token(&mut self) -> u64 {
        let token = self.next_token;
        self.next_token = token.checked_add(1).unwrap_or_else(|| runtime::fatal());
        token
    }

    unsafe fn reject_generated(&self, address: *const u8) -> Result<(), u32> {
        let region = unsafe { <hooks::Backend as Platform>::query(address)? };
        for record in self.records.values() {
            if record
                .retained
                .as_ref()
                .is_some_and(|hook| hook.allocation().cast::<c_void>() == region.AllocationBase)
                || record.open
                    && route(record.route).original.load(Ordering::Acquire) as *mut c_void
                        == region.AllocationBase
            {
                return Err(ERROR_ALREADY_EXISTS);
            }
        }
        Ok(())
    }

    unsafe fn resolve(&self, address: usize) -> Result<usize, u32> {
        let resolve = |address| unsafe {
            neohook::resolve::<hooks::Backend>(address, |address| {
                self.reject_generated(address)?;
                hooks::reject_generated(address)?;
                Ok(hooks::active_target(address))
            })
        };
        let canonical = resolve(address as *const u8)?;
        resolve(protection::attachment_target(canonical)).map(|address| address as usize)
    }
}

struct Shared(UnsafeCell<Registry>);
unsafe impl Sync for Shared {}
static LOCK: runtime::Lock = runtime::Lock::new();
static REGISTRY: Shared = Shared(UnsafeCell::new(Registry {
    records: BTreeMap::new(),
    tokens: Vec::new(),
    next_token: 1,
}));

#[repr(C)]
pub(crate) struct Execution {
    record: u64,
    token: u64,
    pub(crate) callback: usize,
    pub(crate) target: usize,
    route: *const Route,
    bypass: u64,
    lease: reentry::Lease,
}

pub fn attach(
    owner: *mut c_void,
    target: usize,
    callback: usize,
    stack_bytes: u64,
    user_data: u64,
) -> Result<u64, u32> {
    if target == 0
        || callback == 0
        || target == callback
        || stack_bytes > u32::MAX as u64
        || stack_bytes % 8 != 0
    {
        return Err(ERROR_INVALID_PARAMETER);
    }
    reentry::initialize()?;
    coordinator::initialize()?;
    let guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    let target = unsafe { registry.resolve(target)? };
    if target == unsafe { registry.resolve(callback)? } {
        return Err(ERROR_INVALID_PARAMETER);
    }
    let drained = Arc::new(Event::new()?);
    let descriptor = Box::into_raw(Box::new(Route {
        target,
        stack_bytes,
        callback,
        user_data,
        original: AtomicUsize::new(0),
        admission: AtomicUsize::new(0),
        record: AtomicPtr::new(ptr::null_mut()),
    }));
    let id = descriptor as u64;
    let mut record = Box::new(Record {
        owner: Some(owner),
        backend_id: 0,
        route: descriptor,
        open: false,
        retained: None,
        drained: Some(drained),
        cleanup: None,
        application: 0,
    });
    route(descriptor)
        .record
        .store(ptr::from_mut(&mut *record), Ordering::Release);
    let attachment = match hooks::attach_managed(
        owner,
        target as *mut c_void,
        id,
        crate::dispatch::managed_entry as *mut c_void,
        |original| {
            route(descriptor)
                .original
                .store(original as usize, Ordering::Release);
            route(descriptor).admission.store(OPEN, Ordering::Release);
            for provider in coordinator::providers() {
                if provider.canonical == target {
                    provider.active.store(descriptor, Ordering::Release);
                }
            }
        },
    ) {
        Ok(attachment) => attachment,
        Err(failure) => {
            route(descriptor).close();
            if let Some(hook) = failure.hook {
                record.retained = Some(hook);
                record.reclaim();
                let pending = !record.is_drained();
                registry.records.insert(id, record);
                if pending {
                    reentry::reclaim();
                }
                drop(guard);
                wait_drained(owner, id)?;
                let _guard = LOCK.lock();
                unsafe { &mut *REGISTRY.0.get() }.retire(id);
            } else {
                // No provider or ingress has observed a never-published route.
                unsafe { drop(Box::from_raw(descriptor)) };
            }
            return Err(failure.error);
        }
    };
    record.backend_id = attachment.id;
    record.open = true;
    registry.records.insert(id, record);
    Ok(id)
}

pub fn detach(owner: *mut c_void, id: u64) -> Result<(), u32> {
    let _guard = LOCK.lock();
    unsafe { &mut *REGISTRY.0.get() }
        .records
        .get_mut(&id)
        .filter(|record| record.owner == Some(owner) && record.open)
        .ok_or(ERROR_NOT_FOUND)?
        .close()
}

pub fn rollback_attach(owner: *mut c_void, id: u64) -> Result<(), u32> {
    detach(owner, id)?;
    wait_drained(owner, id)?;
    let _guard = LOCK.lock();
    unsafe { &mut *REGISTRY.0.get() }.retire(id);
    Ok(())
}

pub fn is_drained(owner: *mut c_void, id: u64) -> Result<bool, u32> {
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    let record = registry
        .records
        .get_mut(&id)
        .filter(|record| record.owner == Some(owner))
        .ok_or(ERROR_NOT_FOUND)?;
    record.reclaim();
    Ok(record.is_drained())
}

pub fn acquire_original(owner: *mut c_void, id: u64) -> Result<u64, u32> {
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    registry
        .records
        .get(&id)
        .filter(|record| record.owner == Some(owner) && record.open)
        .ok_or(ERROR_NOT_FOUND)?;
    registry
        .tokens
        .try_reserve(1)
        .map_err(|_| windows_sys::Win32::Foundation::ERROR_NOT_ENOUGH_MEMORY)?;
    let token = registry.token();
    registry.tokens.push(Token {
        id: token,
        record: id,
        kind: TokenKind::Original,
    });
    registry.records.get_mut(&id).unwrap().retain_application();
    Ok(token)
}

pub fn release_original(owner: *mut c_void, token: u64) -> Result<(), u32> {
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    let position = registry
        .tokens
        .iter()
        .position(|item| {
            item.id == token
                && matches!(item.kind, TokenKind::Original)
                && registry.records[&item.record].owner == Some(owner)
        })
        .ok_or(ERROR_NOT_FOUND)?;
    let token = registry.tokens.swap_remove(position);
    let record = registry.records.get_mut(&token.record).unwrap();
    record.release_application();
    Ok(())
}

pub fn dispose(owner: *mut c_void) -> Result<(), u32> {
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    for record in registry.records.values_mut() {
        if record.owner == Some(owner) {
            record.close()?;
        }
    }
    let mut index = 0;
    while index < registry.tokens.len() {
        let id = registry.tokens[index].record;
        if registry.records[&id].owner == Some(owner)
            && matches!(registry.tokens[index].kind, TokenKind::Original)
        {
            registry.tokens.swap_remove(index);
            let record = registry.records.get_mut(&id).unwrap();
            record.release_application();
        } else {
            index += 1;
        }
    }
    Ok(())
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureCleanupHookOwner(id: u64) -> u32 {
    let owner = {
        let _guard = LOCK.lock();
        let registry = unsafe { &*REGISTRY.0.get() };
        registry.records.get(&id).and_then(|record| record.owner)
    };
    owner.map_or(ERROR_NOT_FOUND, |owner| cleanup(owner).err().unwrap_or(0))
}

#[cfg(feature = "fixture-api")]
static CLEANUP_EVENT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureSetCleanupEvent(event: HANDLE) {
    CLEANUP_EVENT.store(event, Ordering::Release);
}

#[cfg(feature = "fixture-api")]
static ENTRY_ENTERED: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
#[cfg(feature = "fixture-api")]
static ENTRY_RELEASE: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureSetEntryEvents(entered: HANDLE, release: HANDLE) {
    ENTRY_RELEASE.store(release, Ordering::Relaxed);
    ENTRY_ENTERED.store(entered, Ordering::Release);
}

#[cfg(feature = "fixture-api")]
fn hold_entry() {
    let entered = ENTRY_ENTERED.swap(ptr::null_mut(), Ordering::AcqRel);
    if !entered.is_null() {
        let _suppression = reentry::Suppression::enter();
        let release = ENTRY_RELEASE.load(Ordering::Relaxed);
        let set: SetEvent = unsafe { kernel(c"SetEvent") };
        let wait: WaitEvent = unsafe { kernel(c"WaitForSingleObject") };
        if unsafe { set(entered) } == 0 || unsafe { wait(release, u32::MAX) } != WAIT_OBJECT_0 {
            runtime::fatal();
        }
    }
}

#[cfg(feature = "fixture-api")]
static DRAIN_WAITING: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureSetDrainEvent(event: HANDLE) {
    DRAIN_WAITING.store(event, Ordering::Release);
}

#[cfg(feature = "fixture-api")]
fn notify_drain_wait() {
    let entered = DRAIN_WAITING.swap(ptr::null_mut(), Ordering::AcqRel);
    if !entered.is_null() {
        let set: SetEvent = unsafe { kernel(c"SetEvent") };
        if unsafe { set(entered) } == 0 {
            runtime::fatal();
        }
    }
}

#[cfg(feature = "fixture-api")]
static BYPASS_ROUTE: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "fixture-api")]
static BYPASS_THREAD: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "fixture-api")]
static BYPASS_EVENT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
#[cfg(feature = "fixture-api")]
static BYPASS_WAIT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureSetCleanupBypassEvent(id: u64, thread: u32, event: HANDLE) {
    BYPASS_WAIT.store(ptr::null_mut(), Ordering::Relaxed);
    BYPASS_ROUTE.store(id as usize, Ordering::Relaxed);
    BYPASS_THREAD.store(thread as usize, Ordering::Relaxed);
    BYPASS_EVENT.store(event, Ordering::Release);
}

#[cfg(feature = "fixture-api")]
fn watch_cleanup_wait(event: &Event) {
    if !BYPASS_EVENT.load(Ordering::Acquire).is_null()
        && BYPASS_THREAD.load(Ordering::Relaxed)
            == unsafe { (runtime::api().get_current_thread_id)() } as usize
    {
        BYPASS_WAIT.store(event.0, Ordering::Release);
    }
}

#[cfg(feature = "fixture-api")]
unsafe fn notify_cleanup_bypass(id: u64, frame: *const Frame) {
    if !BYPASS_EVENT.load(Ordering::Acquire).is_null()
        && BYPASS_ROUTE.load(Ordering::Relaxed) == id as usize
        && BYPASS_THREAD.load(Ordering::Relaxed)
            == unsafe { (runtime::api().get_current_thread_id)() } as usize
        && BYPASS_WAIT.load(Ordering::Acquire) as u64 == unsafe { (*frame).registers[1] }
    {
        let event = BYPASS_EVENT.swap(ptr::null_mut(), Ordering::AcqRel);
        if !event.is_null() {
            let set: SetEvent = unsafe { kernel(c"SetEvent") };
            if unsafe { set(event) } == 0 {
                runtime::fatal();
            }
        }
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureManagedRecordCount() -> u64 {
    let _guard = LOCK.lock();
    unsafe { &*REGISTRY.0.get() }.records.len() as u64
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureWaitManagedDrain(id: u64) {
    let _suppression = reentry::Suppression::enter();
    let event = {
        let _guard = LOCK.lock();
        unsafe { &*REGISTRY.0.get() }
            .records
            .get(&id)
            .and_then(|record| record.drained.clone())
    };
    if let Some(event) = event {
        event.wait();
    }
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    if registry.records.contains_key(&id) {
        registry.reclaim(id);
    }
}

pub fn wait_drained(owner: *mut c_void, id: u64) -> Result<(), u32> {
    let _suppression = reentry::Suppression::enter();
    let event = {
        let _guard = LOCK.lock();
        let registry = unsafe { &*REGISTRY.0.get() };
        registry
            .records
            .get(&id)
            .filter(|record| record.owner == Some(owner))
            .ok_or(ERROR_NOT_FOUND)?
            .drained
            .clone()
    };
    #[cfg(feature = "fixture-api")]
    notify_drain_wait();
    if let Some(event) = event {
        event.wait();
    }
    Ok(())
}

pub fn cleanup(owner: *mut c_void) -> Result<(), u32> {
    dispose(owner)?;
    let _suppression = reentry::Suppression::enter();
    #[cfg(feature = "fixture-api")]
    {
        let event = CLEANUP_EVENT.swap(ptr::null_mut(), Ordering::AcqRel);
        if !event.is_null() {
            let set: SetEvent = unsafe { kernel(c"SetEvent") };
            if unsafe { set(event) } == 0 {
                runtime::fatal();
            }
        }
    }
    let ids: Vec<u64> = {
        let _guard = LOCK.lock();
        unsafe { &*REGISTRY.0.get() }
            .records
            .iter()
            .filter_map(|(&id, record)| (record.owner == Some(owner)).then_some(id))
            .collect()
    };
    for id in ids {
        let event = {
            let _guard = LOCK.lock();
            let registry = unsafe { &mut *REGISTRY.0.get() };
            let record = registry.records.get_mut(&id).unwrap();
            let enclosing = reentry::enclosing(record.route.cast());
            if record.application > enclosing {
                let event = Arc::new(Event::new()?);
                record.cleanup = Some(CleanupWait {
                    enclosing,
                    event: event.clone(),
                });
                Some(event)
            } else {
                None
            }
        };
        if let Some(event) = event {
            #[cfg(feature = "fixture-api")]
            {
                watch_cleanup_wait(&event);
                notify_drain_wait();
            }
            event.wait();
        }
        let _guard = LOCK.lock();
        let registry = unsafe { &mut *REGISTRY.0.get() };
        let record = registry.records.get_mut(&id).unwrap();
        record.owner = None;
        registry.reclaim(id);
    }
    Ok(())
}

// FIXME: Reclamation tracks payload references only. A context or return
// address saved outside the payload can replay into released generated code
// after drain. Options: decommit with a retained reservation, quarantine with
// trap bytes, or process-lifetime retention with a generation check.
pub(crate) fn reclaim_pending() {
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    registry.records.retain(|_, record| {
        record.reclaim();
        if record.owner.is_none() && record.is_drained() {
            route(record.route)
                .record
                .store(ptr::null_mut(), Ordering::Release);
            false
        } else {
            true
        }
    });
}

unsafe fn fill(
    execution: *mut Execution,
    record: u64,
    token: u64,
    callback: usize,
    target: usize,
    descriptor: *const Route,
    bypass: bool,
) -> *mut c_void {
    unsafe {
        execution.write(Execution {
            record,
            token,
            callback,
            target,
            route: descriptor,
            bypass: bypass as u64,
            lease: reentry::Lease::new(),
        });
        if !descriptor.is_null() && !bypass {
            (*execution).lease.enter(descriptor.cast());
        }
    }
    execution.cast()
}

/// The ID is supplied only by a prepared native ingress or coordinator redirect.
pub(crate) unsafe extern "system" fn managed_enter(
    id: u64,
    frame: *mut Frame,
    execution: *mut Execution,
) -> *mut c_void {
    #[cfg(feature = "fixture-api")]
    hold_entry();
    let descriptor = route(id as *const Route);
    let bypass = reentry::active();
    let _guard = (!bypass).then(|| LOCK.lock());
    let admitted = descriptor.admit();
    unsafe {
        (*frame).stack_bytes = descriptor.stack_bytes;
        (*frame).user_data = if admitted && !bypass {
            descriptor.user_data
        } else {
            0
        };
        (*frame).token = 0;
    }
    if !admitted {
        return unsafe { fill(execution, 0, 0, 0, descriptor.target, ptr::null(), bypass) };
    }
    if bypass {
        #[cfg(feature = "fixture-api")]
        unsafe {
            notify_cleanup_bypass(id, frame);
        }
        return unsafe {
            fill(
                execution,
                0,
                0,
                0,
                descriptor.original.load(Ordering::Acquire),
                descriptor,
                true,
            )
        };
    }
    let registry = unsafe { &mut *REGISTRY.0.get() };
    registry.records.get_mut(&id).unwrap().application += 1;
    registry
        .tokens
        .try_reserve(1)
        .unwrap_or_else(|_| runtime::fatal());
    let token = registry.token();
    registry.tokens.push(Token {
        id: token,
        record: id,
        kind: TokenKind::Invocation(unsafe { (runtime::api().get_current_thread_id)() }),
    });
    unsafe {
        (*frame).token = token;
        fill(
            execution,
            id,
            token,
            descriptor.callback,
            descriptor.original.load(Ordering::Acquire),
            descriptor,
            false,
        )
    }
}

pub(crate) unsafe extern "system" fn managed_original_enter(
    token: u64,
    frame: *mut Frame,
    execution: *mut Execution,
) -> *mut c_void {
    if frame.is_null() {
        return ptr::null_mut();
    }
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    let thread = unsafe { (runtime::api().get_current_thread_id)() };
    let Some(token) = registry.tokens.iter().find(|item| {
        item.id == token
            && match item.kind {
                TokenKind::Invocation(owner) => owner == thread,
                TokenKind::Original => true,
            }
    }) else {
        return ptr::null_mut();
    };
    let id = token.record;
    // The live token owns a reference, so its descriptor and record remain valid.
    let descriptor = route(id as *const Route);
    if unsafe { (*frame).stack_bytes } != descriptor.stack_bytes
        || descriptor.stack_bytes != 0 && unsafe { (*frame).stack.is_null() }
    {
        return ptr::null_mut();
    }
    registry.records.get_mut(&id).unwrap().retain_application();
    unsafe {
        fill(
            execution,
            id,
            0,
            0,
            descriptor.original.load(Ordering::Acquire),
            descriptor,
            false,
        )
    }
}

pub(crate) unsafe extern "system" fn managed_leave(lease: *mut c_void) {
    let execution = unsafe { &*lease.cast::<Execution>() };
    if !execution.route.is_null() && execution.bypass == 0 {
        unsafe { execution.lease.leave() };
    }
    if execution.bypass != 0 {
        if !execution.route.is_null() && route(execution.route).release() & OPEN == 0 {
            reentry::reclaim();
        }
        return;
    }
    if execution.record == 0 {
        return;
    }
    let _guard = LOCK.lock();
    let registry = unsafe { &mut *REGISTRY.0.get() };
    if execution.token != 0 {
        let position = registry
            .tokens
            .iter()
            .position(|token| token.id == execution.token)
            .unwrap_or_else(|| runtime::fatal());
        registry.tokens.swap_remove(position);
    }
    let descriptor = route(execution.route);
    let record = unsafe { &mut *descriptor.record.load(Ordering::Acquire) };
    record.release_application();
    if record.owner.is_none() && record.is_drained() {
        registry.retire(execution.record);
    }
}
