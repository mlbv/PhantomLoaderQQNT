// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

pub const ENVIRONMENT_VARIABLE: &str = "PHANTOMLOADER_NATIVE_COORDINATOR";
pub const MAGIC: u32 = u32::from_le_bytes(*b"PHPC");
pub const VERSION: u32 = 1;
pub const PATCH_CAPACITY: usize = 32;
pub const REDIRECT_CAPACITY: usize = 4;
pub const ATTACH: u32 = 1;
pub const DETACH: u32 = 2;

/// A leaf gateway transfers control through R10 without changing the native stack.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Redirect {
    pub start: u64,
    pub end: u64,
    pub destination: u64,
    pub token: u64,
}

/// The client installs its resident breakpoint handler before requesting a patch.
/// Its target-specific rendezvous remains closed until this response is received.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Request {
    pub magic: u32,
    pub version: u32,
    pub operation: u32,
    pub length: u32,
    pub target: u64,
    pub before: [u8; PATCH_CAPACITY],
    pub after: [u8; PATCH_CAPACITY],
    pub redirect_count: u32,
    pub reserved: u32,
    pub redirects: [Redirect; REDIRECT_CAPACITY],
}

impl Request {
    pub fn new(operation: u32, target: u64, before: &[u8], after: &[u8]) -> Self {
        let mut request = Self {
            magic: MAGIC,
            version: VERSION,
            operation,
            length: before.len() as u32,
            target,
            before: [0; PATCH_CAPACITY],
            after: [0; PATCH_CAPACITY],
            redirect_count: 0,
            reserved: 0,
            redirects: [Redirect::default(); REDIRECT_CAPACITY],
        };
        request.before[..before.len()].copy_from_slice(before);
        request.after[..after.len()].copy_from_slice(after);
        request
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Response {
    pub magic: u32,
    pub version: u32,
    pub error: u32,
    pub reserved: u32,
}
