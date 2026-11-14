// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::managed::managed_leave;
use core::{
    arch::{asm, global_asm},
    ffi::c_void,
    mem::{align_of, offset_of, size_of},
    ptr,
};
use windows_sys::Win32::System::{
    Diagnostics::Debug::{CONTEXT, DISPATCHER_CONTEXT, EXCEPTION_RECORD},
    Kernel::{EXCEPTION_DISPOSITION, ExceptionContinueSearch},
    SystemServices::{EXCEPTION_EXIT_UNWIND, EXCEPTION_TARGET_UNWIND, EXCEPTION_UNWINDING},
};

/// Native Win64 volatile register state and stack arguments for one managed call.
///
/// Register order is RAX, RCX, RDX, R8, R9, R10, R11. The callback writes native
/// return values into this frame. `stack` points to the first argument after the
/// four register arguments; `stack_bytes` is fixed by the hook configuration.
/// `token` is an opaque original-invocation capability, never a code address.
#[repr(C, align(16))]
pub struct Frame {
    pub registers: [u64; 7],
    pub padding: u64,
    pub xmm: [[u8; 16]; 6],
    pub stack: *mut u8,
    pub stack_bytes: u64,
    pub token: u64,
    pub user_data: u64,
}

const _: () = {
    assert!(size_of::<Frame>() == 192);
    assert!(align_of::<Frame>() == 16);
    assert!(size_of::<crate::managed::Execution>() == 64);
};

global_asm!(
    include_str!("dispatch.s"),
    registers = const offset_of!(Frame, registers),
    padding = const offset_of!(Frame, padding),
    xmm = const offset_of!(Frame, xmm),
    stack = const offset_of!(Frame, stack),
    stack_bytes = const offset_of!(Frame, stack_bytes),
    token = const offset_of!(Frame, token),
    user_data = const offset_of!(Frame, user_data),
    frame_size = const size_of::<Frame>(),
    execution_size = const size_of::<crate::managed::Execution>(),
    execution_callback = const offset_of!(crate::managed::Execution, callback),
    execution_target = const offset_of!(crate::managed::Execution, target),
    managed_enter = sym crate::managed::managed_enter,
    managed_original_enter = sym crate::managed::managed_original_enter,
    managed_leave = sym crate::managed::managed_leave,
    managed_dispatch_unwind = sym managed_dispatch_unwind,
);

unsafe extern "system" {
    /// Entered by native ingress with its resident Route pointer in R10 and RSP intact.
    pub fn managed_entry();

    /// Invokes an original through a live capability; returns 1 for an invalid token.
    pub fn managed_invoke_original(token: u64, frame: *mut Frame) -> u32;
}

unsafe extern "system" fn managed_dispatch_unwind(
    exception: *mut EXCEPTION_RECORD,
    establisher: *const c_void,
    _: *mut CONTEXT,
    dispatcher: *const c_void,
) -> EXCEPTION_DISPOSITION {
    unsafe {
        let flags = (*exception).ExceptionFlags;
        if flags & (EXCEPTION_UNWINDING | EXCEPTION_EXIT_UNWIND) == 0
            || flags & EXCEPTION_TARGET_UNWIND != 0
        {
            return ExceptionContinueSearch;
        }
        let dispatcher = &*dispatcher.cast::<DISPATCHER_CONTEXT>();
        let offset = dispatcher.HandlerData.cast::<u32>().read() as usize;
        let slot = establisher
            .cast_mut()
            .cast::<u8>()
            .add(offset)
            .cast::<*mut c_void>();
        let lease = slot.replace(ptr::null_mut());
        if !lease.is_null() {
            let last_error: u32;
            asm!(
                "mov {last_error:e}, gs:[0x68]",
                last_error = out(reg) last_error,
                options(nostack, readonly, preserves_flags),
            );
            managed_leave(lease);
            asm!(
                "mov gs:[0x68], {last_error:e}",
                last_error = in(reg) last_error,
                options(nostack, preserves_flags),
            );
        }
        ExceptionContinueSearch
    }
}
