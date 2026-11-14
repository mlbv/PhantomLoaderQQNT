// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use std::{
    fmt,
    fs::{File, OpenOptions},
    io::Read,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use serde::{
    Deserializer,
    de::{self, Deserialize, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

use crate::{Result, process::full_path};

const MAX_SELECTOR_BYTES: u64 = 1024 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const SELECTORS: [&str; 5] = [
    "curVersion",
    "baseVersion",
    "buildId",
    "readyVersion",
    "readyInstaller",
];

pub struct TargetProfile<'a> {
    pub qq_version: &'a str,
    pub selector_bases: &'a [platform::profile::SelectorBase],
    pub files: &'a [platform::profile::TargetFile],
}

impl TargetProfile<'static> {
    pub fn supported() -> Self {
        Self {
            qq_version: platform::profile::QQ_VERSION,
            selector_bases: platform::profile::SELECTOR_BASES,
            files: platform::profile::FILES,
        }
    }
}

struct Selectors([Option<String>; 5]);

impl<'de> Deserialize<'de> for Selectors {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Root;
        impl<'de> Visitor<'de> for Root {
            type Value = Selectors;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a version selector object")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Selectors, A::Error> {
                let mut selectors = Selectors(std::array::from_fn(|_| None));
                while let Some(key) = map.next_key::<String>()? {
                    if let Some(index) = SELECTORS.iter().position(|name| *name == key) {
                        if selectors.0[index].is_some() {
                            return Err(de::Error::custom("duplicate version selector"));
                        }
                        selectors.0[index] = Some(map.next_value::<String>()?);
                    } else {
                        let raw = map.next_value::<&RawValue>()?;
                        validate_unknown(raw, 1).map_err(de::Error::custom)?;
                    }
                }
                if selectors.0[..4].iter().any(Option::is_none) {
                    return Err(de::Error::custom("missing version selector"));
                }
                Ok(selectors)
            }
        }
        deserializer.deserialize_map(Root)
    }
}

// Raw numbers retain JSON's full range; strings still undergo Unicode decoding.
fn validate_unknown(raw: &RawValue, depth: usize) -> std::result::Result<(), serde_json::Error> {
    if depth > MAX_JSON_DEPTH {
        return Err(<serde_json::Error as de::Error>::custom(
            "JSON nesting exceeds 64",
        ));
    }
    let text = raw.get();
    let mut deserializer = serde_json::Deserializer::from_str(text);
    match text.as_bytes()[0] {
        b'{' => deserializer.deserialize_map(UnknownContainer(depth))?,
        b'[' => deserializer.deserialize_seq(UnknownContainer(depth))?,
        b'"' => {
            String::deserialize(&mut deserializer)?;
        }
        _ => {
            de::IgnoredAny::deserialize(&mut deserializer)?;
        }
    }
    deserializer.end()
}

struct UnknownContainer(usize);

impl<'de> Visitor<'de> for UnknownContainer {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON container")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        while map.next_key::<String>()?.is_some() {
            map.next_value_seed(UnknownValue(self.0 + 1))?;
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<(), A::Error> {
        while sequence
            .next_element_seed(UnknownValue(self.0 + 1))?
            .is_some()
        {}
        Ok(())
    }
}

struct UnknownValue(usize);

impl<'de> DeserializeSeed<'de> for UnknownValue {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<(), D::Error> {
        let raw = <&RawValue>::deserialize(deserializer)?;
        validate_unknown(raw, self.0).map_err(de::Error::custom)
    }
}

pub fn validate_version_selector(json: &[u8], profile: &TargetProfile<'_>) -> Result<()> {
    const INVALID: &str = "versions/config.json must be valid UTF-8 JSON with unique, top-level curVersion, baseVersion, buildId and readyVersion strings, and an empty readyInstaller string when present";
    if json.is_empty() || json.len() as u64 > MAX_SELECTOR_BYTES {
        return Err(INVALID.into());
    }
    let text = std::str::from_utf8(json).map_err(|_| INVALID.to_owned())?;
    let selectors: Selectors = serde_json::from_str(text).map_err(|_| INVALID.to_owned())?;
    for index in [0, 3, 4] {
        let actual = selectors.0[index].as_deref().unwrap_or("");
        let expected = if index == 4 { "" } else { profile.qq_version };
        if actual != expected && !(index == 3 && actual.is_empty()) {
            return Err(format!(
                "Unsupported versions/config.json selector: {}",
                SELECTORS[index]
            ));
        }
    }
    let base_version = selectors.0[1].as_deref().unwrap();
    let build_id = selectors.0[2].as_deref().unwrap();
    if !profile
        .selector_bases
        .iter()
        .any(|base| base.base_version == base_version && base.build_id == build_id)
    {
        return Err("Unsupported versions/config.json selector: baseVersion/buildId pair".into());
    }
    Ok(())
}

pub struct ValidatedTarget {
    pub executable: PathBuf,
    startup_locks: Vec<File>,
}

impl ValidatedTarget {
    pub fn validate(executable: &Path, profile: &TargetProfile<'_>) -> Result<Self> {
        let executable =
            full_path(executable).map_err(|error| format!("Cannot resolve -exe: {error}"))?;
        if !executable
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("QQ.exe"))
        {
            return Err("PhantomLoaderQQNT requires -exe to name QQ.exe".into());
        }
        let root = executable.parent().unwrap();
        let selector_path = root.join("versions/config.json");
        let mut selector = locked_file(&selector_path)?;
        let selector_size = size(&selector, &selector_path)?;
        if selector_size == 0 || selector_size > MAX_SELECTOR_BYTES {
            return Err("versions/config.json must contain at most 1 MiB of JSON".into());
        }
        let mut json = vec![0; selector_size as usize];
        selector
            .read_exact(&mut json)
            .map_err(|error| format!("Cannot read {}: {error}", selector_path.display()))?;
        validate_version_selector(&json, profile)?;
        let mut startup_locks = vec![selector];
        let mut buffer = vec![0; 1024 * 1024];
        for expected in profile.files {
            let path = root.join(expected.path);
            let mut file = locked_file(&path)?;
            if size(&file, &path)? != expected.bytes {
                return Err(format!("File size mismatch: {}", path.display()));
            }
            let mut hash = Sha256::new();
            loop {
                let read = file
                    .read(&mut buffer)
                    .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
            }
            if format!("{:x}", hash.finalize()) != expected.sha256 {
                return Err(format!("SHA-256 mismatch: {}", path.display()));
            }
            startup_locks.push(file);
        }
        Ok(Self {
            executable,
            startup_locks,
        })
    }

    pub fn release_startup_locks(&mut self) {
        self.startup_locks.clear();
    }
}

fn locked_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|error| format!("Cannot open {}: {error}", path.display()))
}

fn size(file: &File, path: &Path) -> Result<u64> {
    file.metadata()
        .map(|metadata| metadata.len())
        .map_err(|error| format!("Cannot size {}: {error}", path.display()))
}

pub fn read_payload(path: &Path) -> Result<Vec<u8>> {
    let mut file = locked_file(path)?;
    let size = size(&file, path)?;
    if size == 0 || size > 16 * 1024 * 1024 {
        return Err("payload size out of range".into());
    }
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes)
        .map_err(|error| format!("ReadFile(payload) failed: {error}"))?;
    Ok(bytes)
}
