// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

#![no_std]

extern crate alloc;

mod capture;
mod coordinator;
mod dispatch;
mod foreign;
mod formatting;
mod hooks;
mod loader;
mod managed;
mod napi;
mod native;
mod protection;
mod reentry;
pub(crate) mod runtime;

use core::{ffi::c_void, panic::PanicInfo};
use platform::context::{BootstrapContext, PAYLOAD_ABI_VERSION, PAYLOAD_MAGIC};
use windows_sys::Win32::Foundation::HMODULE;

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    runtime::fatal()
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn PayloadMain(parameter: *mut c_void) -> u32 {
    unsafe {
        let context = parameter.cast::<BootstrapContext>();
        if context.is_null()
            || (*context).magic != PAYLOAD_MAGIC
            || (*context).version != PAYLOAD_ABI_VERSION
        {
            return 0;
        }
        if !runtime::initialize(context) || protection::initialize().is_err() {
            return 0;
        }
        loader::install();
        1
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn InitializePayloadRuntime(context: *mut BootstrapContext) -> bool {
    unsafe { runtime::initialize(context) }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DisposeNativeHooks(owner: *mut c_void) -> u32 {
    hooks::dispose(owner).err().unwrap_or(0)
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn FixtureManagedAttach(
    target: *mut c_void,
    callback: *mut c_void,
    stack_bytes: u64,
    user_data: u64,
    id: *mut u64,
) -> u32 {
    match managed::attach(
        1usize as *mut c_void,
        target as usize,
        callback as usize,
        stack_bytes,
        user_data,
    ) {
        Ok(value) => {
            unsafe { id.write(value) };
            0
        }
        Err(error) => error,
    }
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureManagedDetach(id: u64) -> u32 {
    managed::detach(1usize as *mut c_void, id)
        .err()
        .unwrap_or(0)
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureManagedWaitDrained(id: u64) -> u32 {
    managed::wait_drained(1usize as *mut c_void, id)
        .err()
        .unwrap_or(0)
}

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureManagedInvokeOriginal() -> usize {
    dispatch::managed_invoke_original as *const () as usize
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn RegisterNativeApi(
    module: HMODULE,
    context: *mut BootstrapContext,
) -> bool {
    unsafe { runtime::initialize(context) && native::register(module) }
}
