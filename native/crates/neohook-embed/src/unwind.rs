// SPDX-FileCopyrightText: 2026 NeoHook Authors
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later
use crate::{Platform, Result};
use core::{marker::PhantomData, slice};
use iced_x86::{
    Decoder, DecoderOptions, FlowControl, InstructionInfoFactory, InstructionInfoOptions, OpAccess,
    Register,
};
use neohook_core::disasm::RelocationMapping;
use windows_sys::Win32::{
    Foundation::ERROR_NOT_SUPPORTED, System::Diagnostics::Debug::IMAGE_RUNTIME_FUNCTION_ENTRY,
};

const UNWIND_HEADER_SIZE: usize = 4;
const MAX_UNWIND_SLOTS: usize = u8::MAX as usize;
pub(crate) const MAX_TABLE_SIZE: usize = size_of::<IMAGE_RUNTIME_FUNCTION_ENTRY>()
    + UNWIND_HEADER_SIZE
    + MAX_UNWIND_SLOTS.next_multiple_of(2) * size_of::<u16>();

pub(crate) struct Unwind<P: Platform> {
    entry: *const IMAGE_RUNTIME_FUNCTION_ENTRY,
    marker: PhantomData<P>,
}

impl<P: Platform> Unwind<P> {
    pub unsafe fn register(
        target: *const u8,
        region: *mut u8,
        area: usize,
        mapping: &RelocationMapping,
        stolen: usize,
    ) -> Result<Option<Self>> {
        let mut image_base = 0u64;
        let function = unsafe { P::lookup_function(target as u64, &mut image_base)? };
        if function.is_null() {
            return Ok(None);
        }
        let function = unsafe { &*function };
        let info = (image_base + unsafe { function.Anonymous.UnwindData } as u64) as *const u8;
        let delta = target as u64 - (image_base + function.BeginAddress as u64);
        let codes = unsafe { neohook_core::unwind::translate(info, delta, mapping, stolen)? };
        if codes.slots.is_empty() {
            return Ok(None);
        }
        let prolog_end =
            image_base + function.BeginAddress as u64 + u64::from(unsafe { info.add(1).read() });
        let frame_register = unsafe { info.add(3).read() } & 0xf;
        unsafe { validate_frame_prefix(target, stolen, prolog_end, frame_register)? };
        let info_offset = area + size_of::<IMAGE_RUNTIME_FUNCTION_ENTRY>();
        let header = unsafe { region.add(info_offset) };
        unsafe {
            // A relocated jump back retains its source frame; version 1 would
            // mistake that transfer for a tail-call epilogue and skip its unwind codes.
            header.write(2);
            header.add(1).write(codes.prolog_size);
            header.add(2).write(codes.slots.len() as u8);
            header.add(3).write(codes.frame);
            for (index, value) in codes.slots.iter().enumerate() {
                header
                    .add(UNWIND_HEADER_SIZE + index * size_of::<u16>())
                    .cast::<u16>()
                    .write_unaligned(*value);
            }
            if codes.slots.len() % 2 != 0 {
                header
                    .add(UNWIND_HEADER_SIZE + codes.slots.len() * size_of::<u16>())
                    .cast::<u16>()
                    .write_unaligned(0);
            }
        }
        let entry = unsafe { region.add(area).cast::<IMAGE_RUNTIME_FUNCTION_ENTRY>() };
        unsafe {
            entry.write(core::mem::zeroed());
            (*entry).BeginAddress = 0;
            (*entry).EndAddress = mapping.written_len as u32;
            (*entry).Anonymous.UnwindData = info_offset as u32;
            P::add_function(entry, region as u64)?;
        }
        Ok(Some(Self {
            entry,
            marker: PhantomData,
        }))
    }
}

/// Generated frames retain their translated prologue state through the transfer.
/// Copied body instructions that change that state need explicit epilogue translation.
unsafe fn validate_frame_prefix(
    target: *const u8,
    stolen: usize,
    prolog_end: u64,
    frame_register: u8,
) -> Result<()> {
    let bytes = unsafe { slice::from_raw_parts(target, stolen) };
    let decoder = Decoder::with_ip(64, bytes, target as u64, DecoderOptions::NONE);
    let mut information = InstructionInfoFactory::new();
    for instruction in decoder {
        if instruction.ip() < prolog_end {
            continue;
        }
        match instruction.flow_control() {
            FlowControl::Call | FlowControl::IndirectCall => continue,
            FlowControl::Return => return Err(ERROR_NOT_SUPPORTED),
            _ => {}
        }
        let writes_frame = information
            .info_options(&instruction, InstructionInfoOptions::NO_MEMORY_USAGE)
            .used_registers()
            .iter()
            .any(|used| {
                let register = used.register().full_register();
                matches!(
                    used.access(),
                    OpAccess::Write
                        | OpAccess::CondWrite
                        | OpAccess::ReadWrite
                        | OpAccess::ReadCondWrite
                ) && (register == Register::RSP
                    || frame_register != 0
                        && register.is_gpr64()
                        && register.number() == usize::from(frame_register))
            });
        if writes_frame {
            return Err(ERROR_NOT_SUPPORTED);
        }
    }
    Ok(())
}

impl<P: Platform> Drop for Unwind<P> {
    fn drop(&mut self) {
        unsafe { P::delete_function(self.entry) };
    }
}
