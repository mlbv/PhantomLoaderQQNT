// SPDX-FileCopyrightText: 2026 NeoHook Authors
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later
#![no_std]

extern crate alloc;

#[cfg(not(all(windows, target_arch = "x86_64")))]
compile_error!("This NeoHook embedding requires Windows x64");

mod encoding;
mod inline;
mod memory;
mod read;
mod resolve;
mod unwind;

pub use inline::InlineHook;
pub use resolve::resolve;
use windows_sys::Win32::System::{
    Diagnostics::Debug::IMAGE_RUNTIME_FUNCTION_ENTRY, Memory::MEMORY_BASIC_INFORMATION,
    SystemInformation::SYSTEM_INFO,
};

pub type Result<T> = core::result::Result<T, u32>;

/// All callbacks must operate on this process. They must not allocate or acquire
/// application locks in query/protect/flush, which run while threads are frozen.
pub unsafe trait Platform {
    unsafe fn query(address: *const u8) -> Result<MEMORY_BASIC_INFORMATION>;
    unsafe fn allocate(address: *const u8, size: usize) -> *mut u8;
    unsafe fn free(address: *mut u8);
    unsafe fn protect(address: *const u8, size: usize, protection: u32) -> Result<u32>;
    unsafe fn flush(address: *const u8, size: usize) -> Result<()>;
    fn system_info() -> Result<SYSTEM_INFO>;
    unsafe fn lookup_function(
        address: u64,
        base: &mut u64,
    ) -> Result<*const IMAGE_RUNTIME_FUNCTION_ENTRY>;
    unsafe fn add_function(entry: *const IMAGE_RUNTIME_FUNCTION_ENTRY, base: u64) -> Result<()>;
    unsafe fn delete_function(entry: *const IMAGE_RUNTIME_FUNCTION_ENTRY);
    fn fatal() -> !;
}
