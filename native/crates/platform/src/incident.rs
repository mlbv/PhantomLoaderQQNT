// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

include!(concat!(env!("OUT_DIR"), "/incident.rs"));

#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
    Uninitialized = STATE_UNINITIALIZED,
    WaitingForWrapper = STATE_WAITING,
    Armed = STATE_ARMED,
    Failed = STATE_FAILED,
}

pub const fn needs_report(exit_code: u32) -> bool {
    matches!(exit_code, CAPTURED | DUMP_FAILED | ARM_FAILED)
}
