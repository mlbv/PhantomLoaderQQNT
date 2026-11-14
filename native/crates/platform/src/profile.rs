// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

#[derive(Clone, Copy)]
pub struct TargetFile {
    pub path: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Clone, Copy)]
pub struct SelectorBase {
    pub base_version: &'static str,
    pub build_id: &'static str,
}

#[derive(Clone, Copy)]
pub struct HookSite {
    pub rva: usize,
    pub prologue: &'static [u8],
}

#[derive(Clone, Copy)]
pub struct ImagePatch {
    pub rva: usize,
    pub expected: &'static [u8],
    pub replacement: &'static [u8],
}

#[derive(Clone, Copy)]
pub struct InspectionProfile {
    pub time_date_stamp: u32,
    pub image_size: u32,
    pub patches: &'static [ImagePatch],
}

#[derive(Clone, Copy)]
pub struct ForeignStringLayout {
    pub bytes: usize,
    pub long_flag_mask: u8,
    pub long_length_offset: usize,
    pub long_data_offset: usize,
    pub short_data_offset: usize,
    pub short_length_shift: u32,
}

#[derive(Clone, Copy)]
pub struct TaskLayout {
    pub kind_offset: usize,
    pub program_offset: usize,
    pub program_types: &'static [u32],
    pub string: ForeignStringLayout,
}

include!(concat!(env!("OUT_DIR"), "/profile.rs"));
