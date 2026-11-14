// SPDX-FileCopyrightText: 2026 NeoHook Authors
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later
use crate::{Platform, Result};
use alloc::vec::Vec;
use core::{marker::PhantomData, ptr};
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY};
use windows_sys::Win32::System::Memory::{MEM_FREE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE};

pub(crate) struct Allocation<P: Platform> {
    pub address: *mut u8,
    pub size: usize,
    marker: PhantomData<P>,
}

impl<P: Platform> Allocation<P> {
    pub fn nearby(target: *const u8, size: usize) -> Result<Self> {
        let system = P::system_info()?;
        let granularity = system.dwAllocationGranularity as usize;
        let radius = i32::MAX as usize - size - crate::encoding::MAX_INSTRUCTION_LEN;
        let lower = (target as usize)
            .saturating_sub(radius)
            .max(system.lpMinimumApplicationAddress as usize)
            .max(granularity);
        let upper = (target as usize)
            .saturating_add(radius)
            .min(system.lpMaximumApplicationAddress as usize);
        let mut candidates = Vec::new();
        let mut cursor = lower;
        while cursor < upper {
            let region = unsafe { P::query(cursor as *const u8)? };
            let start = region.BaseAddress as usize;
            let end = start + region.RegionSize;
            if region.State == MEM_FREE
                && let Some(candidate) = nearest(
                    start.max(lower),
                    end.min(upper),
                    target as usize,
                    size,
                    granularity,
                )
            {
                candidates
                    .try_reserve(1)
                    .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
                candidates.push(candidate);
            }
            cursor = end;
        }
        candidates.sort_unstable_by_key(|address| address.abs_diff(target as usize));
        for address in candidates {
            let allocation = unsafe { P::allocate(address as *const u8, size) };
            if !allocation.is_null() {
                return Ok(Self {
                    address: allocation,
                    size,
                    marker: PhantomData,
                });
            }
        }
        Err(ERROR_NOT_ENOUGH_MEMORY)
    }

    pub fn seal(&self) -> Result<()> {
        unsafe {
            P::protect(self.address, self.size, PAGE_EXECUTE_READ)?;
            P::flush(self.address, self.size)
        }
    }
}

impl<P: Platform> Drop for Allocation<P> {
    fn drop(&mut self) {
        unsafe { P::free(self.address) };
    }
}

fn nearest(
    lower: usize,
    upper: usize,
    target: usize,
    size: usize,
    alignment: usize,
) -> Option<usize> {
    let high = upper.checked_sub(size)?;
    let first = lower.next_multiple_of(alignment);
    let last = high / alignment * alignment;
    if first > last {
        return None;
    }
    let ideal = target.clamp(first, last);
    [
        ideal / alignment * alignment,
        ideal.next_multiple_of(alignment),
    ]
    .into_iter()
    .filter(|address| *address >= first && *address <= last)
    .min_by_key(|address| address.abs_diff(target))
}

#[derive(Clone, Copy)]
struct Page {
    address: *const u8,
    size: usize,
}

pub(crate) struct Patch {
    pages: [Page; 2],
    count: usize,
}

impl Patch {
    pub unsafe fn prepare<P: Platform>(address: *const u8, size: usize) -> Result<Self> {
        let mut patch = Self {
            pages: [Page {
                address: ptr::null(),
                size: 0,
            }; 2],
            count: 0,
        };
        let end = address as usize + size;
        let mut cursor = address as usize;
        while cursor < end {
            let region = unsafe { P::query(cursor as *const u8)? };
            let next = end.min(region.BaseAddress as usize + region.RegionSize);
            patch.pages[patch.count] = Page {
                address: cursor as *const u8,
                size: next - cursor,
            };
            patch.count += 1;
            cursor = next;
        }
        Ok(patch)
    }

    /// On error the original bytes and page protections have been restored.
    /// The caller must suspend all threads that could execute this range.
    pub unsafe fn replace<P: Platform>(
        &self,
        address: *mut u8,
        before: &[u8],
        after: &[u8],
    ) -> Result<()> {
        if unsafe { core::slice::from_raw_parts(address, before.len()) } != before {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let mut protections = [0u32; 2];
        for (index, page) in self.pages[..self.count].iter().enumerate() {
            match unsafe { P::protect(page.address, page.size, PAGE_EXECUTE_READWRITE) } {
                Ok(old) => protections[index] = old,
                Err(error) => {
                    for undo in (0..index).rev() {
                        let page = self.pages[undo];
                        if unsafe { P::protect(page.address, page.size, protections[undo]) }
                            .is_err()
                        {
                            P::fatal();
                        }
                    }
                    return Err(error);
                }
            }
        }
        unsafe { copy(address, after) };
        let result = unsafe { P::flush(address, after.len()) }.and_then(|()| {
            for (page, old) in self.pages[..self.count].iter().zip(protections) {
                unsafe { P::protect(page.address, page.size, old)? };
            }
            Ok(())
        });
        if let Err(error) = result {
            for page in &self.pages[..self.count] {
                if unsafe { P::protect(page.address, page.size, PAGE_EXECUTE_READWRITE) }.is_err() {
                    P::fatal();
                }
            }
            unsafe { copy(address, before) };
            if unsafe { P::flush(address, before.len()) }.is_err() {
                P::fatal();
            }
            for (page, old) in self.pages[..self.count].iter().zip(protections) {
                if unsafe { P::protect(page.address, page.size, old) }.is_err() {
                    P::fatal();
                }
            }
            return Err(error);
        }
        Ok(())
    }
}

unsafe fn copy(destination: *mut u8, bytes: &[u8]) {
    for (index, byte) in bytes.iter().enumerate() {
        unsafe { destination.add(index).write_volatile(*byte) };
    }
}
