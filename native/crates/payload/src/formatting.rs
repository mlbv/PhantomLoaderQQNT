// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use core::fmt::{self, Write};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, MAX_PATH},
    Storage::FileSystem::{
        FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS,
    },
};

use crate::runtime;

pub struct Text<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Text<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(self.as_bytes()) }
    }
}

impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.length + value.len();
        self.bytes[self.length..end].copy_from_slice(value.as_bytes());
        self.length = end;
        Ok(())
    }
}

pub fn log(message: fmt::Arguments<'_>) {
    unsafe {
        let api = runtime::api();
        let mut path = [0u16; MAX_PATH as usize];
        let length = (api.get_temp_path_w)(path.len() as u32, path.as_mut_ptr()) as usize;
        let name = "PhantomLoaderQQNT.log";
        if length == 0 || length + name.len() >= path.len() {
            return;
        }
        for (index, byte) in name.bytes().enumerate() {
            path[length + index] = u16::from(byte);
        }
        let file = (api.create_file_w)(
            path.as_ptr(),
            FILE_APPEND_DATA,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            core::ptr::null_mut(),
        );
        if file == INVALID_HANDLE_VALUE {
            return;
        }
        let mut text = alloc::string::String::new();
        let _ = writeln!(text, "[{}] {message}", (api.get_current_process_id)());
        let mut written = 0;
        (api.write_file)(
            file,
            text.as_bytes().as_ptr().cast(),
            text.as_bytes().len() as u32,
            &mut written,
            core::ptr::null_mut(),
        );
        (api.close_handle)(file);
    }
}
