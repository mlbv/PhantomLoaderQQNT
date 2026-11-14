// SPDX-FileCopyrightText: 2026 NeoHook Authors
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later
use crate::{Platform, Result};
use core::marker::PhantomData;
use windows_sys::Win32::{
    Foundation::ERROR_INVALID_PARAMETER,
    System::Memory::{
        MEM_COMMIT, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD,
        PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
    },
};

pub(crate) struct Memory<P>(PhantomData<P>);

unsafe impl<P: Platform> neohook_core::Memory for Memory<P> {
    unsafe fn read<T: Copy>(address: *const u8) -> Result<T> {
        let region = unsafe { P::query(address)? };
        let end = (address as usize)
            .checked_add(size_of::<T>())
            .ok_or(ERROR_INVALID_PARAMETER)?;
        if region.State != MEM_COMMIT
            || region.Protect & (PAGE_NOACCESS | PAGE_GUARD) != 0
            || end > region.BaseAddress as usize + region.RegionSize
        {
            return Err(ERROR_INVALID_PARAMETER);
        }
        Ok(unsafe { address.cast::<T>().read_unaligned() })
    }

    unsafe fn readable_bytes(address: *const u8) -> usize {
        const READABLE: u32 = PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY;
        let Ok(region) = (unsafe { P::query(address) }) else {
            return 0;
        };
        if region.State != MEM_COMMIT
            || (region.Protect & READABLE) == 0
            || (region.Protect & (PAGE_GUARD | PAGE_NOACCESS)) != 0
        {
            return 0;
        }
        let end = (region.BaseAddress as usize).saturating_add(region.RegionSize);
        end.saturating_sub(address as usize)
    }
}
