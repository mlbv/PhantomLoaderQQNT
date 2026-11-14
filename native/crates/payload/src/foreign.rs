// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use core::{ffi::c_void, ptr};
use platform::profile::TASK_LAYOUT;

pub struct Program {
    pub kind: u32,
    pub data: *const c_void,
    pub bytes: usize,
}

pub unsafe fn task_program(task: *const u8) -> Option<Program> {
    unsafe {
        let layout = TASK_LAYOUT;
        let kind = ptr::read_unaligned(task.add(layout.kind_offset).cast::<u32>());
        if !layout.program_types.contains(&kind) {
            return None;
        }
        let string = task.add(layout.program_offset);
        let discriminator = *string;
        let (data, bytes) = if discriminator & layout.string.long_flag_mask != 0 {
            (
                ptr::read_unaligned(
                    string
                        .add(layout.string.long_data_offset)
                        .cast::<*const c_void>(),
                ),
                ptr::read_unaligned(string.add(layout.string.long_length_offset).cast::<usize>()),
            )
        } else {
            (
                string.add(layout.string.short_data_offset).cast(),
                (discriminator >> layout.string.short_length_shift) as usize,
            )
        };
        Some(Program { kind, data, bytes })
    }
}
