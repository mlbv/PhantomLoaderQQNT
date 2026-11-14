// SPDX-FileCopyrightText: 2026 NeoHook Authors
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later
use crate::read::Memory;
use crate::{
    Platform, Result,
    memory::{Allocation, Patch},
    unwind::Unwind,
};
use alloc::vec::Vec;
use core::{mem::ManuallyDrop, slice};
use iced_x86::{Decoder, DecoderOptions, FlowControl};
use neohook_core::disasm::{Disassembler, RelocationMapping};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_SUPPORTED,
};

use crate::encoding::{
    ABSOLUTE_JUMP_LEN, INDIRECT_JUMP, MAX_RELOCATED_LEN, NOP, RELATIVE_JUMP_LEN,
    RELATIVE_JUMP_OPCODE,
};

const MANAGED_GATEWAY_LEN: usize = 2 + size_of::<u64>() + ABSOLUTE_JUMP_LEN;

enum Destination {
    Direct(*const u8),
    Managed { id: u64, dispatcher: *const u8 },
}

struct TrampolineLayout {
    code_capacity: usize,
    relay_offset: usize,
    gateway_offset: Option<usize>,
    unwind_offset: usize,
    allocation_size: usize,
}

impl TrampolineLayout {
    fn new(stolen: usize, managed: bool) -> Self {
        // At most one instruction per stolen byte, plus the jump back.
        let code_capacity = (stolen + 1) * MAX_RELOCATED_LEN;
        let relay_offset = code_capacity;
        let gateway_offset = managed.then_some(relay_offset + ABSOLUTE_JUMP_LEN);
        let code_end = gateway_offset.map_or(relay_offset + ABSOLUTE_JUMP_LEN, |offset| {
            offset + MANAGED_GATEWAY_LEN
        });
        let unwind_offset = code_end.next_multiple_of(core::mem::align_of::<
            windows_sys::Win32::System::Diagnostics::Debug::IMAGE_RUNTIME_FUNCTION_ENTRY,
        >());
        Self {
            code_capacity,
            relay_offset,
            gateway_offset,
            unwind_offset,
            allocation_size: unwind_offset + crate::unwind::MAX_TABLE_SIZE,
        }
    }
}

/// Prepared inline patch. Dropping this value never changes live code or frees
/// generated code; release it explicitly after a successful detach and caller
/// quiescence, or after an attachment transaction was rolled back.
pub struct InlineHook<P: Platform> {
    target: *mut u8,
    original: Vec<u8>,
    replacement: Vec<u8>,
    mapping: RelocationMapping,
    protection: Patch,
    allocation: ManuallyDrop<Allocation<P>>,
    unwind: ManuallyDrop<Option<Unwind<P>>>,
    relay: bool,
    layout: TrampolineLayout,
}

impl<P: Platform> InlineHook<P> {
    /// Chained unwind metadata and relocated exception-handler-protected body
    /// instructions return ERROR_NOT_SUPPORTED; prologue-only copies are supported.
    ///
    /// # Safety
    /// The target must be a canonical function entry with stable, nonoverlapping code.
    /// Its overwritten prefix covers complete instructions totaling at least five bytes.
    /// While patched, control flow from outside that prefix may enter only at its first
    /// byte, including transfers through branches, returns and restored contexts.
    /// The caller establishes this property; preparation does not prove it.
    /// Both addresses and all referenced code must remain resident until release.
    pub unsafe fn prepare(target: *const u8, replacement: *const u8) -> Result<Self> {
        unsafe { Self::prepare_inner(target, Destination::Direct(replacement)) }
    }

    /// The generated leaf passes the managed token in R10 without changing the stack.
    ///
    /// # Safety
    /// The target must satisfy [`Self::prepare`]'s entry and code-stability requirements.
    /// The canonical target and resident dispatcher must remain live until release.
    pub unsafe fn prepare_managed(
        target: *const u8,
        id: u64,
        dispatcher: *const u8,
    ) -> Result<Self> {
        unsafe { Self::prepare_inner(target, Destination::Managed { id, dispatcher }) }
    }

    unsafe fn prepare_inner(target: *const u8, destination: Destination) -> Result<Self> {
        let len =
            unsafe { Disassembler::<Memory<P>>::get_instruction_len(target, RELATIVE_JUMP_LEN) }
                .map_err(|_| ERROR_INVALID_PARAMETER)?;
        // This best-effort scan does not establish the caller's single-entry precondition.
        if unsafe { Disassembler::<Memory<P>>::branches_into(target, len) } {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let layout = TrampolineLayout::new(len, matches!(destination, Destination::Managed { .. }));
        let allocation = Allocation::<P>::nearby(target, layout.allocation_size)?;
        let mapping = unsafe {
            Disassembler::<Memory<P>>::relocate(
                target,
                allocation.address,
                len,
                layout.code_capacity,
            )
        }
        .map_err(|_| ERROR_INVALID_PARAMETER)?;
        let (destination, relay) = match destination {
            Destination::Direct(replacement) => {
                let relative = replacement as i64 - target as i64 - RELATIVE_JUMP_LEN as i64;
                let relay = i32::try_from(relative).is_err();
                let address = if relay {
                    let address = unsafe { allocation.address.add(layout.relay_offset) };
                    unsafe {
                        address
                            .cast::<[u8; INDIRECT_JUMP.len()]>()
                            .write(INDIRECT_JUMP);
                        address
                            .add(INDIRECT_JUMP.len())
                            .cast::<u64>()
                            .write_unaligned(replacement as u64);
                    }
                    address as *const u8
                } else {
                    replacement
                };
                (address, relay)
            }
            Destination::Managed { id, dispatcher } => {
                let offset = layout.relay_offset + ABSOLUTE_JUMP_LEN;
                let address = unsafe { allocation.address.add(offset) };
                unsafe {
                    address.cast::<[u8; 2]>().write([0x49, 0xba]);
                    address.add(2).cast::<u64>().write_unaligned(id);
                    address
                        .add(10)
                        .cast::<[u8; INDIRECT_JUMP.len()]>()
                        .write(INDIRECT_JUMP);
                    address
                        .add(10 + INDIRECT_JUMP.len())
                        .cast::<u64>()
                        .write_unaligned(dispatcher as u64);
                }
                (address as *const u8, false)
            }
        };
        let displacement =
            i32::try_from(destination as i64 - target as i64 - RELATIVE_JUMP_LEN as i64)
                .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        let mut original = Vec::new();
        original
            .try_reserve_exact(len)
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        original.extend_from_slice(unsafe { slice::from_raw_parts(target, len) });
        let mut patched = Vec::new();
        patched
            .try_reserve_exact(len)
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        patched.resize(len, NOP);
        patched[0] = RELATIVE_JUMP_OPCODE;
        patched[1..RELATIVE_JUMP_LEN].copy_from_slice(&displacement.to_le_bytes());
        let protection = unsafe { Patch::prepare::<P>(target, len)? };
        let unwind = unsafe {
            Unwind::<P>::register(
                target,
                allocation.address,
                layout.unwind_offset,
                &mapping,
                len,
            )?
        };
        allocation.seal()?;
        Ok(Self {
            target: target.cast_mut(),
            original,
            replacement: patched,
            mapping,
            protection,
            allocation: ManuallyDrop::new(allocation),
            unwind: ManuallyDrop::new(unwind),
            relay,
            layout,
        })
    }

    pub fn target(&self) -> *const u8 {
        self.target
    }
    pub fn original_bytes(&self) -> &[u8] {
        &self.original
    }
    pub fn patched_bytes(&self) -> &[u8] {
        &self.replacement
    }
    pub fn original(&self) -> *mut u8 {
        self.allocation.address
    }
    pub fn allocation(&self) -> *const u8 {
        self.allocation.address
    }
    pub fn gateway_range(&self) -> Option<(usize, usize)> {
        self.layout.gateway_offset.map(|offset| {
            let start = self.allocation.address as usize + offset;
            (start, start + MANAGED_GATEWAY_LEN)
        })
    }

    /// A call already in flight before attachment must not return into the
    /// overwritten span and enter generated code without a managed lease.
    pub fn validate_managed_activation(&self) -> Result<()> {
        let end = self.target as usize + self.original.len();
        let decoder =
            Decoder::with_ip(64, &self.original, self.target as u64, DecoderOptions::NONE);
        for instruction in decoder {
            if matches!(
                instruction.flow_control(),
                FlowControl::Call | FlowControl::IndirectCall
            ) && instruction.next_ip() < end as u64
            {
                return Err(ERROR_NOT_SUPPORTED);
            }
        }
        Ok(())
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        let start = self.target as usize;
        let other_start = other.target as usize;
        start < other_start + other.original.len() && other_start < start + self.original.len()
    }

    /// Validates exact instruction boundaries without modifying any state.
    pub fn map_ip(&self, pc: usize, attaching: bool) -> Result<Option<usize>> {
        let target = self.target as usize;
        let trampoline = self.allocation.address as usize;
        if attaching && (target..target + self.original.len()).contains(&pc) {
            let index = self
                .mapping
                .old_instruction_offsets
                .iter()
                .position(|offset| target + *offset as usize == pc)
                .ok_or(ERROR_INVALID_PARAMETER)?;
            return Ok(Some(
                trampoline + self.mapping.new_instruction_offsets[index] as usize,
            ));
        }
        if !attaching {
            if (target..target + self.original.len()).contains(&pc) {
                return Ok(Some(target));
            }
            if (trampoline..trampoline + self.mapping.written_len).contains(&pc) {
                if pc == trampoline + self.mapping.jmp_back_offset as usize {
                    return Ok(Some(target + self.original.len()));
                }
                let index = self
                    .mapping
                    .new_instruction_offsets
                    .iter()
                    .position(|offset| trampoline + *offset as usize == pc)
                    .ok_or(ERROR_INVALID_PARAMETER)?;
                return Ok(Some(
                    target + self.mapping.old_instruction_offsets[index] as usize,
                ));
            }
            if self.relay
                && (trampoline + self.layout.relay_offset
                    ..trampoline + self.layout.relay_offset + ABSOLUTE_JUMP_LEN)
                    .contains(&pc)
            {
                if pc != trampoline + self.layout.relay_offset {
                    return Err(ERROR_INVALID_PARAMETER);
                }
                return Ok(Some(target));
            }
        }
        Ok(None)
    }

    /// All threads that could execute the patch must be suspended, and their
    /// instruction pointers must be mapped before resuming them.
    pub unsafe fn write(&self, attaching: bool) -> Result<()> {
        let (before, after) = if attaching {
            (&self.original, &self.replacement)
        } else {
            (&self.replacement, &self.original)
        };
        unsafe { self.protection.replace::<P>(self.target, before, after) }
    }

    /// The patch must be inactive and all trampoline/replacement callers joined.
    pub unsafe fn release(mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.unwind);
            ManuallyDrop::drop(&mut self.allocation);
        }
    }
}
