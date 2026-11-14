// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::{
    Result,
    process::{Handle, SuspendedProcess, ThreadContext, wide, windows_error},
};
use platform::coordinator::{
    ATTACH, DETACH, MAGIC, PATCH_CAPACITY, REDIRECT_CAPACITY, Request, Response, VERSION,
};
use std::{
    ffi::{OsStr, OsString, c_void},
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::Arc,
    thread::JoinHandle,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, ERROR_INVALID_DATA,
        ERROR_INVALID_PARAMETER, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_NO_MORE_FILES,
        ERROR_NOT_SUPPORTED, ERROR_PARTIAL_COPY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
        ERROR_RETRY, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{
        FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile,
        WriteFile,
    },
    System::{
        Diagnostics::{
            Debug::{
                CONTEXT_CONTROL_AMD64, CONTEXT_INTEGER_AMD64, FlushInstructionCache,
                GetThreadContext, ReadProcessMemory, SetThreadContext, WriteProcessMemory,
            },
            ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
        },
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        LibraryLoader::{GetModuleHandleA, GetProcAddress},
        Memory::{
            MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, VirtualProtectEx,
            VirtualQueryEx,
        },
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        },
        Threading::{
            CreateEventW, GetCurrentProcess, GetProcessIdOfThread, INFINITE, OpenThread,
            ResetEvent, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
            THREAD_QUERY_LIMITED_INFORMATION, THREAD_SET_CONTEXT, THREAD_SUSPEND_RESUME,
            TerminateProcess, WaitForMultipleObjects, WaitForSingleObject,
        },
    },
};

type WinResult<T> = std::result::Result<T, u32>;
type CreateState = unsafe extern "system" fn(*mut HANDLE, u32, *const c_void, HANDLE, u32) -> i32;
type ChangeState = unsafe extern "system" fn(HANDLE, HANDLE, u32, *mut c_void, usize, u32) -> i32;
type StatusError = unsafe extern "system" fn(i32) -> u32;

struct Native {
    create: CreateState,
    change: ChangeState,
    error: StatusError,
}

impl Native {
    fn resolve() -> Result<Self> {
        let module = unsafe { GetModuleHandleA(c"ntdll.dll".as_ptr().cast()) };
        let export = |name: &std::ffi::CStr| unsafe {
            GetProcAddress(module, name.as_ptr().cast()).ok_or_else(|| {
                format!("Windows 11 coordinator requires {}", name.to_string_lossy())
            })
        };
        Ok(Self {
            create: unsafe { std::mem::transmute(export(c"NtCreateProcessStateChange")?) },
            change: unsafe { std::mem::transmute(export(c"NtChangeProcessState")?) },
            error: unsafe { std::mem::transmute(export(c"RtlNtStatusToDosError")?) },
        })
    }

    fn status(&self, status: i32) -> WinResult<()> {
        if status < 0 {
            Err(unsafe { (self.error)(status) })
        } else {
            Ok(())
        }
    }
}

#[cfg(feature = "testhost")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestPhase {
    GateFlushed,
    PeersStopped,
    BeforeCommit,
    BeforeResponse(u32),
}

#[cfg(feature = "testhost")]
pub type TestObserver =
    Arc<dyn Fn(TestPhase, HANDLE, &Request) -> std::result::Result<(), u32> + Send + Sync>;

/// Prepared before child creation so injection-time hooks can connect immediately.
pub struct PreparedCoordinator {
    name: OsString,
    pipe: Handle,
    native: Native,
    #[cfg(feature = "testhost")]
    observer: Option<TestObserver>,
}

impl PreparedCoordinator {
    pub fn prepare() -> Result<Self> {
        let native = Native::resolve()?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = OsString::from(format!(
            r"\\.\pipe\PhantomLoaderQQNT-{}-{nonce:x}",
            std::process::id()
        ));
        let encoded = wide(&name)?;
        let pipe = unsafe {
            CreateNamedPipeW(
                encoded.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                size_of::<Response>() as u32,
                size_of::<Request>() as u32,
                0,
                null(),
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return Err(windows_error("CreateNamedPipeW(coordinator)"));
        }
        Ok(Self {
            name,
            pipe: unsafe { Handle::from_raw(pipe) },
            native,
            #[cfg(feature = "testhost")]
            observer: None,
        })
    }

    #[cfg(feature = "testhost")]
    pub fn observe(mut self, observer: TestObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Starts before payload injection; owns a duplicate of the target process handle.
    pub fn start(self, process: &SuspendedProcess) -> Result<RunningCoordinator> {
        let mut duplicate = null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                process.handle.raw(),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(windows_error("DuplicateHandle(coordinator target)"));
        }
        let target = Arc::new(unsafe { Handle::from_raw(duplicate) });
        let worker_target = target.clone();
        let pid = process.id;
        let worker = std::thread::Builder::new()
            .name("native-coordinator".into())
            .spawn(move || {
                let result = self.serve(worker_target.raw(), pid);
                if result.is_err() {
                    terminate(worker_target.raw());
                }
                result
            })
            .map_err(|error| format!("Cannot start native coordinator: {error}"))?;
        Ok(RunningCoordinator {
            target,
            worker: Some(worker),
        })
    }

    fn serve(&self, target: HANDLE, pid: u32) -> Result<()> {
        let mut io = PipeIo::new(self.pipe.raw(), target)?;
        loop {
            let connected = io.connect();
            match connected {
                Ok(false) => return Ok(()),
                Ok(true) => {}
                Err(error) => return Err(format!("Coordinator connect failed: {error}")),
            }
            let mut client = 0;
            if unsafe { GetNamedPipeClientProcessId(self.pipe.raw(), &mut client) } == 0 {
                let error = unsafe { GetLastError() };
                if !disconnected(error) {
                    return Err(format!("GetNamedPipeClientProcessId failed: {error}"));
                }
            } else if client == pid {
                loop {
                    let mut request: Request = unsafe { zeroed() };
                    let bytes = unsafe {
                        std::slice::from_raw_parts_mut(
                            (&mut request as *mut Request).cast::<u8>(),
                            size_of::<Request>(),
                        )
                    };
                    match io.transfer(bytes, false) {
                        Ok(false) => return Ok(()),
                        Err(error) if disconnected(error) => break,
                        Err(error) => return Err(format!("Coordinator read failed: {error}")),
                        Ok(true) => {}
                    }
                    let error = match validate(&request) {
                        Err(error) => error,
                        Ok(()) => patch(
                            &self.native,
                            target,
                            pid,
                            &request,
                            #[cfg(feature = "testhost")]
                            self.observer.as_ref(),
                        )?,
                    };
                    #[cfg(feature = "testhost")]
                    if let Some(observer) = &self.observer {
                        observer(TestPhase::BeforeResponse(error), target, &request)
                            .map_err(|error| format!("Coordinator observer failed: {error}"))?;
                    }
                    let mut response = Response {
                        magic: MAGIC,
                        version: VERSION,
                        error,
                        reserved: 0,
                    };
                    let bytes = unsafe {
                        std::slice::from_raw_parts_mut(
                            (&mut response as *mut Response).cast::<u8>(),
                            size_of::<Response>(),
                        )
                    };
                    match io.transfer(bytes, true) {
                        Ok(false) => return Ok(()),
                        Err(error) if disconnected(error) => break,
                        Err(error) => return Err(format!("Coordinator write failed: {error}")),
                        Ok(true) => {}
                    }
                }
            }
            if unsafe { DisconnectNamedPipe(self.pipe.raw()) } == 0 {
                let error = unsafe { GetLastError() };
                if !disconnected(error) {
                    return Err(format!("DisconnectNamedPipe failed: {error}"));
                }
            }
        }
    }
}

pub struct RunningCoordinator {
    target: Arc<Handle>,
    worker: Option<JoinHandle<Result<()>>>,
}

impl RunningCoordinator {
    /// Call after the target exits, including the reporter and -wait paths.
    pub fn finish(mut self) -> Result<()> {
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| "Native coordinator panicked")?
    }
}

impl Drop for RunningCoordinator {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            terminate(self.target.raw());
            let _ = worker.join();
        }
    }
}

fn terminate(target: HANDLE) {
    // A failed broker must never leave an unserviced rendezvous or suspended peers.
    if unsafe { TerminateProcess(target, 0xC000_0001) } == 0
        && unsafe { WaitForSingleObject(target, 0) } != WAIT_OBJECT_0
    {
        std::process::abort();
    }
}

fn disconnected(error: u32) -> bool {
    matches!(
        error,
        ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED
    )
}

struct PipeIo {
    pipe: HANDLE,
    target: HANDLE,
    event: Handle,
}

impl PipeIo {
    fn new(pipe: HANDLE, target: HANDLE) -> Result<Self> {
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(windows_error("CreateEventW(coordinator I/O)"));
        }
        Ok(Self {
            pipe,
            target,
            event: unsafe { Handle::from_raw(event) },
        })
    }

    fn operation(&self) -> WinResult<OVERLAPPED> {
        if unsafe { ResetEvent(self.event.raw()) } == 0 {
            return Err(unsafe { GetLastError() });
        }
        Ok(OVERLAPPED {
            hEvent: self.event.raw(),
            ..unsafe { zeroed() }
        })
    }

    fn complete(&self, started: i32, operation: &mut OVERLAPPED) -> WinResult<Option<u32>> {
        if started == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_PENDING {
                return Err(error);
            }
            let handles = [self.target, self.event.raw()];
            let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if wait != WAIT_OBJECT_0 + 1 {
                let error = unsafe { GetLastError() };
                // The OVERLAPPED and buffer cannot go out of scope until cancellation completes.
                unsafe {
                    CancelIoEx(self.pipe, operation);
                }
                let mut ignored = 0;
                unsafe {
                    GetOverlappedResult(self.pipe, operation, &mut ignored, 1);
                }
                return if wait == WAIT_OBJECT_0 {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
        }
        let mut transferred = 0;
        if unsafe { GetOverlappedResult(self.pipe, operation, &mut transferred, 0) } == 0 {
            return Err(unsafe { GetLastError() });
        }
        Ok(Some(transferred))
    }

    fn connect(&mut self) -> WinResult<bool> {
        let mut operation = self.operation()?;
        let started = unsafe { ConnectNamedPipe(self.pipe, &mut operation) };
        if started == 0 && unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
            return Ok(true);
        }
        self.complete(started, &mut operation)
            .map(|value| value.is_some())
    }

    fn transfer(&mut self, mut bytes: &mut [u8], write: bool) -> WinResult<bool> {
        while !bytes.is_empty() {
            let mut operation = self.operation()?;
            let started = unsafe {
                if write {
                    WriteFile(
                        self.pipe,
                        bytes.as_ptr(),
                        bytes.len() as u32,
                        null_mut(),
                        &mut operation,
                    )
                } else {
                    ReadFile(
                        self.pipe,
                        bytes.as_mut_ptr(),
                        bytes.len() as u32,
                        null_mut(),
                        &mut operation,
                    )
                }
            };
            match self.complete(started, &mut operation)? {
                None => return Ok(false),
                Some(0) => return Err(ERROR_BROKEN_PIPE),
                Some(count) => bytes = &mut bytes[count as usize..],
            }
        }
        Ok(true)
    }
}

fn range(start: u64, length: u64) -> WinResult<std::ops::Range<u64>> {
    match start.checked_add(length) {
        Some(end) if start != 0 && end > start && end <= isize::MAX as u64 => Ok(start..end),
        _ => Err(ERROR_INVALID_PARAMETER),
    }
}

fn validate(request: &Request) -> WinResult<()> {
    let length = request.length as usize;
    let count = request.redirect_count as usize;
    if request.magic != MAGIC
        || request.version != VERSION
        || !matches!(request.operation, ATTACH | DETACH)
        || length == 0
        || length > PATCH_CAPACITY
        || count > REDIRECT_CAPACITY
        || request.reserved != 0
        || request.operation == ATTACH && count != 0
        || request.before[length..]
            .iter()
            .chain(&request.after[length..])
            .any(|&byte| byte != 0)
        || request.redirects[count..].iter().any(|redirect| {
            redirect.start != 0
                || redirect.end != 0
                || redirect.destination != 0
                || redirect.token != 0
        })
    {
        return Err(ERROR_INVALID_PARAMETER);
    }
    if request.before[0] == 0xcc || request.after[0] == 0xcc {
        return Err(ERROR_NOT_SUPPORTED);
    }
    let patch = range(request.target, request.length as u64)?;
    for (index, redirect) in request.redirects[..count].iter().enumerate() {
        let length = redirect
            .end
            .checked_sub(redirect.start)
            .ok_or(ERROR_INVALID_PARAMETER)?;
        let span = range(redirect.start, length)?;
        range(redirect.destination, 1)?;
        if span.start < patch.end && patch.start < span.end
            || request.redirects[..index]
                .iter()
                .any(|previous| span.start < previous.end && previous.start < span.end)
        {
            return Err(ERROR_INVALID_PARAMETER);
        }
    }
    Ok(())
}

struct Freeze<'a> {
    native: &'a Native,
    state: Handle,
    target: HANDLE,
    suspended: bool,
}

impl<'a> Freeze<'a> {
    fn new(native: &'a Native, target: HANDLE) -> WinResult<Self> {
        let mut state = null_mut();
        native.status(unsafe { (native.create)(&mut state, 1, null(), target, 0) })?;
        let mut freeze = Self {
            native,
            state: unsafe { Handle::from_raw(state) },
            target,
            suspended: false,
        };
        native
            .status(unsafe { (native.change)(freeze.state.raw(), target, 0, null_mut(), 0, 0) })?;
        freeze.suspended = true;
        Ok(freeze)
    }

    fn resume(&mut self) -> WinResult<()> {
        if self.suspended {
            self.native.status(unsafe {
                (self.native.change)(self.state.raw(), self.target, 1, null_mut(), 0, 0)
            })?;
            self.suspended = false;
        }
        Ok(())
    }
}

struct Region {
    start: usize,
    length: usize,
    protection: u32,
    writable: bool,
}

struct Peer {
    thread: Handle,
    original: ThreadContext,
    changed: bool,
    suspended: bool,
}

struct Transaction<'a> {
    target: HANDLE,
    request: &'a Request,
    freeze: Freeze<'a>,
    regions: Vec<Region>,
    peers: Vec<Peer>,
    touched: bool,
    committed: bool,
    active: bool,
    #[cfg(feature = "testhost")]
    observer: Option<&'a TestObserver>,
}

fn patch(
    native: &Native,
    target: HANDLE,
    pid: u32,
    request: &Request,
    #[cfg(feature = "testhost")] observer: Option<&TestObserver>,
) -> Result<u32> {
    let freeze = match Freeze::new(native, target) {
        Ok(freeze) => freeze,
        Err(error) => return Ok(error),
    };
    let mut transaction = Transaction {
        target,
        request,
        freeze,
        regions: Vec::new(),
        peers: Vec::new(),
        touched: false,
        committed: false,
        active: true,
        #[cfg(feature = "testhost")]
        observer,
    };
    let result = transaction.apply(pid);
    if let Err(error) = result {
        if transaction.committed {
            return Err(format!("Native patch failed after entry commit: {error}"));
        }
        transaction
            .rollback()
            .map_err(|error| format!("Native patch rollback failed: {error}"))?;
    }
    transaction
        .release()
        .map_err(|error| format!("Native patch resume failed: {error}"))?;
    transaction.active = false;
    Ok(result.err().unwrap_or(0))
}

impl Transaction<'_> {
    #[cfg(feature = "testhost")]
    fn observe(&self, phase: TestPhase) -> WinResult<()> {
        match self.observer {
            Some(observer) => observer(phase, self.target, self.request),
            None => Ok(()),
        }
    }

    fn apply(&mut self, pid: u32) -> WinResult<()> {
        let request = self.request;
        let mut actual = [0; PATCH_CAPACITY];
        read(
            self.target,
            request.target as usize,
            &mut actual[..request.length as usize],
        )?;
        if actual[..request.length as usize] != request.before[..request.length as usize] {
            return Err(ERROR_INVALID_DATA);
        }
        self.query_regions()?;
        self.make_writable()?;
        self.touched = true;
        write(self.target, request.target as usize, &[0xcc])?;
        self.flush()?;
        #[cfg(feature = "testhost")]
        self.observe(TestPhase::GateFlushed)?;
        // Freeze does not stop bypass threads. Only the resident entry rendezvous
        // prevents new ABI entries while the post-INT3 snapshot is suspended.
        self.stop_peers(pid)?;
        #[cfg(feature = "testhost")]
        self.observe(TestPhase::PeersStopped)?;
        self.write_suffix(&request.after[..request.length as usize])?;
        for peer in &mut self.peers {
            let mut context = peer.original;
            let rip = context.0.Rip;
            if (request.target..request.target + request.length as u64).contains(&rip) {
                context.0.Rip = request.target;
            } else if let Some(redirect) = request.redirects[..request.redirect_count as usize]
                .iter()
                .find(|redirect| (redirect.start..redirect.end).contains(&rip))
            {
                context.0.Rip = redirect.destination;
                context.0.R10 = redirect.token;
            } else {
                continue;
            }
            peer.changed = true;
            if unsafe { SetThreadContext(peer.thread.raw(), &context.0) } == 0 {
                return Err(unsafe { GetLastError() });
            }
        }
        #[cfg(feature = "testhost")]
        self.observe(TestPhase::BeforeCommit)?;
        // A bypass thread may enter as soon as the first byte is visible, even
        // before WriteProcessMemory returns. No recoverable error follows this point.
        self.committed = true;
        write(self.target, request.target as usize, &request.after[..1])?;
        self.flush()?;
        self.restore_protections()
    }

    fn query_regions(&mut self) -> WinResult<()> {
        let mut address = self.request.target as usize;
        let end = address + self.request.length as usize;
        while address < end {
            let mut info: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
            if unsafe {
                VirtualQueryEx(
                    self.target,
                    address as *const c_void,
                    &mut info,
                    size_of_val(&info),
                )
            } == 0
            {
                return Err(unsafe { GetLastError() });
            }
            if info.State != MEM_COMMIT {
                return Err(ERROR_INVALID_DATA);
            }
            let segment_end = end.min(info.BaseAddress as usize + info.RegionSize);
            self.regions.push(Region {
                start: address,
                length: segment_end - address,
                protection: info.Protect,
                writable: false,
            });
            address = segment_end;
        }
        Ok(())
    }

    fn make_writable(&mut self) -> WinResult<()> {
        for region in &mut self.regions {
            if !region.writable {
                let mut old = 0;
                if unsafe {
                    VirtualProtectEx(
                        self.target,
                        region.start as *const c_void,
                        region.length,
                        PAGE_EXECUTE_READWRITE,
                        &mut old,
                    )
                } == 0
                {
                    return Err(unsafe { GetLastError() });
                }
                region.writable = true;
            }
        }
        Ok(())
    }

    fn restore_protections(&mut self) -> WinResult<()> {
        for region in &mut self.regions {
            if region.writable {
                let mut old = 0;
                if unsafe {
                    VirtualProtectEx(
                        self.target,
                        region.start as *const c_void,
                        region.length,
                        region.protection,
                        &mut old,
                    )
                } == 0
                {
                    return Err(unsafe { GetLastError() });
                }
                region.writable = false;
            }
        }
        Ok(())
    }

    fn stop_peers(&mut self, pid: u32) -> WinResult<()> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(unsafe { GetLastError() });
        }
        let snapshot = unsafe { Handle::from_raw(snapshot) };
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut found = unsafe { Thread32First(snapshot.raw(), &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                self.stop_peer(entry.th32ThreadID, pid)?;
            }
            found = unsafe { Thread32Next(snapshot.raw(), &mut entry) };
        }
        if unsafe { GetLastError() } != ERROR_NO_MORE_FILES {
            return Err(unsafe { GetLastError() });
        }
        Ok(())
    }

    fn stop_peer(&mut self, id: u32, pid: u32) -> WinResult<()> {
        let thread = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME
                    | THREAD_GET_CONTEXT
                    | THREAD_SET_CONTEXT
                    | THREAD_QUERY_LIMITED_INFORMATION
                    | 0x0010_0000,
                0,
                id,
            )
        };
        if thread.is_null() {
            let error = unsafe { GetLastError() };
            // A bypass peer can exit between the snapshot and OpenThread.
            return if error == ERROR_INVALID_PARAMETER {
                Ok(())
            } else {
                Err(error)
            };
        }
        let thread = unsafe { Handle::from_raw(thread) };
        if unsafe { GetProcessIdOfThread(thread.raw()) } != pid {
            return Ok(());
        }
        if unsafe { SuspendThread(thread.raw()) } == u32::MAX {
            let error = unsafe { GetLastError() };
            return if unsafe { WaitForSingleObject(thread.raw(), 0) } == WAIT_OBJECT_0 {
                Ok(())
            } else {
                Err(error)
            };
        }
        self.peers.push(Peer {
            thread,
            original: ThreadContext(unsafe { zeroed() }),
            changed: false,
            suspended: true,
        });
        let peer = self.peers.last_mut().unwrap();
        peer.original.0.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
        if unsafe { GetThreadContext(peer.thread.raw(), &mut peer.original.0) } == 0 {
            let error = unsafe { GetLastError() };
            if unsafe { WaitForSingleObject(peer.thread.raw(), 0) } == WAIT_OBJECT_0 {
                peer.suspended = false;
                return Ok(());
            }
            return Err(error);
        }
        if self.request.operation == ATTACH
            && (self.request.target..self.request.target + self.request.length as u64)
                .contains(&peer.original.0.Rip)
        {
            return Err(ERROR_RETRY);
        }
        Ok(())
    }

    fn flush(&self) -> WinResult<()> {
        if unsafe {
            FlushInstructionCache(
                self.target,
                self.request.target as *const c_void,
                self.request.length as usize,
            )
        } == 0
        {
            return Err(unsafe { GetLastError() });
        }
        Ok(())
    }

    fn write_suffix(&self, bytes: &[u8]) -> WinResult<()> {
        if bytes.len() > 1 {
            write(self.target, self.request.target as usize + 1, &bytes[1..])?;
        }
        self.flush()
    }

    fn write_patch(&self, bytes: &[u8]) -> WinResult<()> {
        self.write_suffix(bytes)?;
        write(self.target, self.request.target as usize, &bytes[..1])?;
        self.flush()
    }

    fn rollback(&mut self) -> WinResult<()> {
        if self.touched {
            self.make_writable()?;
            self.write_patch(&self.request.before[..self.request.length as usize])?;
        }
        for peer in &mut self.peers {
            if peer.changed {
                if unsafe { SetThreadContext(peer.thread.raw(), &peer.original.0) } == 0 {
                    return Err(unsafe { GetLastError() });
                }
                peer.changed = false;
            }
        }
        self.restore_protections()
    }

    fn release(&mut self) -> WinResult<()> {
        for peer in &mut self.peers {
            if peer.suspended {
                if unsafe { ResumeThread(peer.thread.raw()) } == u32::MAX {
                    let error = unsafe { GetLastError() };
                    if unsafe { WaitForSingleObject(peer.thread.raw(), 0) } != WAIT_OBJECT_0 {
                        return Err(error);
                    }
                }
                peer.suspended = false;
            }
        }
        self.freeze.resume()
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if self.active {
            terminate(self.target);
        }
    }
}

fn read(target: HANDLE, address: usize, bytes: &mut [u8]) -> WinResult<()> {
    let mut count = 0;
    if unsafe {
        ReadProcessMemory(
            target,
            address as *const c_void,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut count,
        )
    } == 0
    {
        return Err(unsafe { GetLastError() });
    }
    if count != bytes.len() {
        return Err(ERROR_PARTIAL_COPY);
    }
    Ok(())
}

fn write(target: HANDLE, address: usize, bytes: &[u8]) -> WinResult<()> {
    let mut count = 0;
    if unsafe {
        WriteProcessMemory(
            target,
            address as *const c_void,
            bytes.as_ptr().cast(),
            bytes.len(),
            &mut count,
        )
    } == 0
    {
        return Err(unsafe { GetLastError() });
    }
    if count != bytes.len() {
        return Err(ERROR_PARTIAL_COPY);
    }
    Ok(())
}
