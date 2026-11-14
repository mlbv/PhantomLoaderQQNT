// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::mem::size_of;

use windows_sys::Win32::System::{
    Diagnostics::Debug::{
        IMAGE_DIRECTORY_ENTRY_BASERELOC, IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT,
        IMAGE_DIRECTORY_ENTRY_IMPORT, IMAGE_DIRECTORY_ENTRY_TLS, IMAGE_NT_HEADERS64,
        IMAGE_NT_OPTIONAL_HDR64_MAGIC, IMAGE_OPTIONAL_HEADER64, IMAGE_SCN_MEM_EXECUTE,
        IMAGE_SCN_MEM_WRITE, IMAGE_SECTION_HEADER,
    },
    Memory::{PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_READONLY, PAGE_READWRITE},
    SystemInformation::IMAGE_FILE_MACHINE_AMD64,
    SystemServices::{
        IMAGE_BASE_RELOCATION, IMAGE_DOS_HEADER, IMAGE_DOS_SIGNATURE, IMAGE_NT_SIGNATURE,
        IMAGE_REL_BASED_ABSOLUTE, IMAGE_REL_BASED_DIR64,
    },
};

use crate::Result;

#[derive(Clone, Copy, Debug)]
pub struct Section {
    pub rva: usize,
    pub raw_offset: usize,
    pub raw_size: usize,
    pub virtual_size: usize,
    pub characteristics: u32,
}

impl Section {
    pub fn memory_size(&self) -> usize {
        if self.virtual_size == 0 {
            self.raw_size
        } else {
            self.virtual_size
        }
    }

    pub fn copy_size(&self) -> usize {
        self.raw_size.min(self.memory_size())
    }

    pub fn protection(&self) -> u32 {
        match (
            self.characteristics & IMAGE_SCN_MEM_EXECUTE != 0,
            self.characteristics & IMAGE_SCN_MEM_WRITE != 0,
        ) {
            (true, true) => PAGE_EXECUTE_READWRITE,
            (true, false) => PAGE_EXECUTE_READ,
            (false, true) => PAGE_READWRITE,
            (false, false) => PAGE_READONLY,
        }
    }
}

pub struct Payload {
    file: Vec<u8>,
    pub image_size: usize,
    pub headers_size: usize,
    pub sections: Vec<Section>,
    image_base: u64,
    entry_rva: usize,
    reloc_rva: usize,
    reloc_size: usize,
}

impl Payload {
    pub fn parse(file: Vec<u8>) -> Result<Self> {
        let dos: IMAGE_DOS_HEADER = read(&file, 0).ok_or("payload is too small")?;
        if dos.e_magic != IMAGE_DOS_SIGNATURE || dos.e_lfanew < 0 {
            return Err("payload has no valid PE header".into());
        }
        let nt_offset = dos.e_lfanew as usize;
        let nt: IMAGE_NT_HEADERS64 =
            read(&file, nt_offset).ok_or("payload has no valid PE header")?;
        if nt.Signature != IMAGE_NT_SIGNATURE
            || nt.OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC
            || nt.FileHeader.Machine != IMAGE_FILE_MACHINE_AMD64
        {
            return Err("payload is not an x64 PE image".into());
        }
        let optional = nt.OptionalHeader;
        for index in [
            IMAGE_DIRECTORY_ENTRY_IMPORT,
            IMAGE_DIRECTORY_ENTRY_TLS,
            IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT,
        ] {
            if optional.DataDirectory[index as usize].VirtualAddress != 0 {
                return Err("payload must have no imports or TLS".into());
            }
        }
        let headers_size = optional.SizeOfHeaders as usize;
        let image_size = optional.SizeOfImage as usize;
        if headers_size > file.len() || image_size < headers_size {
            return Err("payload image sizes are invalid".into());
        }
        let table_offset = nt_offset
            + std::mem::offset_of!(IMAGE_NT_HEADERS64, OptionalHeader)
            + nt.FileHeader.SizeOfOptionalHeader as usize;
        let section_count = nt.FileHeader.NumberOfSections as usize;
        let table_size = section_count * size_of::<IMAGE_SECTION_HEADER>();
        if (nt.FileHeader.SizeOfOptionalHeader as usize) < size_of::<IMAGE_OPTIONAL_HEADER64>()
            || section_count == 0
            || !contains(file.len(), table_offset, table_size)
            || !contains(headers_size, table_offset, table_size)
        {
            return Err("payload section table or extents are invalid".into());
        }
        let mut sections = Vec::with_capacity(section_count);
        for index in 0..section_count {
            let header: IMAGE_SECTION_HEADER = read(
                &file,
                table_offset + index * size_of::<IMAGE_SECTION_HEADER>(),
            )
            .unwrap();
            let section = Section {
                rva: header.VirtualAddress as usize,
                raw_offset: header.PointerToRawData as usize,
                raw_size: header.SizeOfRawData as usize,
                virtual_size: unsafe { header.Misc.VirtualSize } as usize,
                characteristics: header.Characteristics,
            };
            if section.raw_size != 0 && !contains(file.len(), section.raw_offset, section.raw_size)
                || !contains(image_size, section.rva, section.raw_size)
                || !contains(image_size, section.rva, section.virtual_size)
            {
                return Err("payload section table or extents are invalid".into());
            }
            sections.push(section);
        }
        let entry_rva = optional.AddressOfEntryPoint as usize;
        if entry_rva == 0 || entry_rva >= image_size {
            return Err("payload entry point is outside the image".into());
        }
        let relocation = optional.DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC as usize];
        Ok(Self {
            file,
            image_size,
            headers_size,
            sections,
            image_base: optional.ImageBase,
            entry_rva,
            reloc_rva: relocation.VirtualAddress as usize,
            reloc_size: relocation.Size as usize,
        })
    }

    pub fn entry_address(&self, image_base: usize) -> usize {
        image_base + self.entry_rva
    }

    pub fn map_at(&self, image_base: u64) -> Result<Vec<u8>> {
        let mut mapped = vec![0; self.image_size];
        mapped[..self.headers_size].copy_from_slice(&self.file[..self.headers_size]);
        for section in &self.sections {
            if section.raw_size != 0 {
                mapped[section.rva..section.rva + section.raw_size].copy_from_slice(
                    &self.file[section.raw_offset..section.raw_offset + section.raw_size],
                );
            }
        }
        self.relocate(&mut mapped, image_base)?;
        Ok(mapped)
    }

    fn relocate(&self, mapped: &mut [u8], image_base: u64) -> Result<()> {
        let invalid = || "payload relocation directory is invalid or unsupported".to_owned();
        let delta = image_base.wrapping_sub(self.image_base);
        if delta == 0 {
            return Ok(());
        }
        if self.reloc_rva == 0 || !contains(mapped.len(), self.reloc_rva, self.reloc_size) {
            return Err(invalid());
        }
        let mut offset = self.reloc_rva;
        let end = offset + self.reloc_size;
        while offset < end {
            if end - offset < size_of::<IMAGE_BASE_RELOCATION>() {
                return Err(invalid());
            }
            let block: IMAGE_BASE_RELOCATION = read(mapped, offset).unwrap();
            let block_size = block.SizeOfBlock as usize;
            let header_size = size_of::<IMAGE_BASE_RELOCATION>();
            if block_size < header_size
                || block_size > end - offset
                || (block_size - header_size) % size_of::<u16>() != 0
            {
                return Err(invalid());
            }
            let entries = (block_size - header_size) / size_of::<u16>();
            for index in 0..entries {
                let entry: u16 =
                    read(mapped, offset + header_size + index * size_of::<u16>()).unwrap();
                let kind = (entry >> 12) as u32;
                if kind == IMAGE_REL_BASED_ABSOLUTE {
                    continue;
                }
                if kind != IMAGE_REL_BASED_DIR64 {
                    return Err(invalid());
                }
                let rva = block.VirtualAddress as usize + (entry & 0xfff) as usize;
                let value: u64 = read(mapped, rva).ok_or_else(invalid)?;
                mapped[rva..rva + size_of::<u64>()]
                    .copy_from_slice(&value.wrapping_add(delta).to_le_bytes());
            }
            offset += block_size;
        }
        Ok(())
    }
}

fn contains(length: usize, offset: usize, size: usize) -> bool {
    offset <= length && size <= length - offset
}

// All callers read integer-only PE records; PE offsets need not be aligned.
fn read<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    if !contains(bytes.len(), offset, size_of::<T>()) {
        return None;
    }
    Some(unsafe { bytes.as_ptr().add(offset).cast::<T>().read_unaligned() })
}
