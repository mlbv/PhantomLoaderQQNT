// SPDX-FileCopyrightText: 2026 NeoHook Authors
// SPDX-License-Identifier: MIT OR Apache-2.0
pub(crate) const MAX_INSTRUCTION_LEN: usize = 15;
pub(crate) const RELATIVE_JUMP_OPCODE: u8 = 0xe9;
pub(crate) const RELATIVE_JUMP_LEN: usize = 1 + size_of::<i32>();
pub(crate) const INDIRECT_JUMP: [u8; 6] = [0xff, 0x25, 0, 0, 0, 0];
pub(crate) const ABSOLUTE_JUMP_LEN: usize = INDIRECT_JUMP.len() + size_of::<u64>();
pub(crate) const NOP: u8 = 0x90;

// A get-PC call expands to push imm32; mov dword [rsp+4], imm32; jmp.
// This also bounds a block-encoder branch and its indirect target literal.
pub(crate) const MAX_RELOCATED_LEN: usize =
    (1 + size_of::<i32>()) + (4 + size_of::<i32>()) + ABSOLUTE_JUMP_LEN;
