// SPDX-FileCopyrightText: 2026 NeoHook Authors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::{Platform, Result, read::Memory};

/// Resolves NeoHook's supported leading jump/import stubs. The visitor can stop
/// at a known active patch, whose jump must not be followed into its replacement.
/// All code and indirect slots must stay resident throughout resolution.
pub unsafe fn resolve<P: Platform>(
    pointer: *const u8,
    stop: impl FnMut(*const u8) -> Result<bool>,
) -> Result<*const u8> {
    unsafe { neohook_core::resolve::<Memory<P>>(pointer, stop) }
}
