// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{env, fmt::Write, fs, path::PathBuf};

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .expect("profile field must be an unsigned integer")
}

fn rva(value: &Value) -> usize {
    let text = value.as_str().expect("RVA must be a hexadecimal string");
    usize::from_str_radix(text.strip_prefix("0x").expect("RVA must begin with 0x"), 16)
        .expect("invalid hexadecimal RVA")
}

fn signature(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .expect("signature must be a string")
        .split_ascii_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).expect("invalid signature byte"))
        .collect()
}

fn hook(output: &mut String, name: &str, value: &Value) {
    writeln!(
        output,
        "pub const {name}: HookSite = HookSite {{ rva: {:#x}, prologue: &{:?} }};",
        rva(&value["rva"]),
        signature(&value["prologue"])
    )
    .unwrap();
}

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../..");
    let path = root.join("distribution/profile.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = fs::read(path).expect("read distribution profile");
    let profile: Value = serde_json::from_slice(&bytes).expect("parse distribution profile");
    assert_eq!(profile["platform"], "win32");
    assert_eq!(profile["architecture"], "x64");
    let mut output = String::new();
    for (name, field) in [
        ("PLATFORM_VERSION", "platformVersion"),
        ("QQ_VERSION", "qqVersion"),
        ("BUILD_VERSION", "buildVersion"),
        (
            "LITELOADER_COMPATIBILITY_VERSION",
            "liteloaderCompatibilityVersion",
        ),
    ] {
        writeln!(
            output,
            "pub const {name}: &str = {:?};",
            profile[field].as_str().unwrap()
        )
        .unwrap();
    }
    writeln!(
        output,
        "pub const PROFILE_SHA256: &str = \"{:x}\";",
        Sha256::digest(&bytes)
    )
    .unwrap();
    writeln!(
        output,
        "pub const NATIVE_API_VERSION: u32 = {};",
        number(&profile["nativeApiVersion"])
    )
    .unwrap();
    writeln!(output, "pub const SELECTOR_BASES: &[SelectorBase] = &[").unwrap();
    for base in profile["selectorBases"].as_array().unwrap() {
        writeln!(
            output,
            "SelectorBase {{ base_version: {:?}, build_id: {:?} }},",
            base["baseVersion"].as_str().unwrap(),
            base["buildId"].as_str().unwrap()
        )
        .unwrap();
    }
    writeln!(output, "];").unwrap();
    writeln!(output, "pub const FILES: &[TargetFile] = &[").unwrap();
    for file in profile["files"].as_array().unwrap() {
        writeln!(
            output,
            "TargetFile {{ path: {:?}, bytes: {}, sha256: {:?} }},",
            file["path"].as_str().unwrap(),
            number(&file["bytes"]),
            file["sha256"].as_str().unwrap()
        )
        .unwrap();
    }
    writeln!(output, "];").unwrap();
    let wrapper = &profile["native"]["wrapper"];
    let wrapper_file = profile["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"].as_str().unwrap().ends_with("/wrapper.node"))
        .unwrap();
    writeln!(
        output,
        "pub const WRAPPER_SHA256: &str = {:?};",
        wrapper_file["sha256"].as_str().unwrap()
    )
    .unwrap();
    writeln!(
        output,
        "pub const WRAPPER_TIMESTAMP: u32 = {:#x};",
        rva(&wrapper["timeDateStamp"])
    )
    .unwrap();
    writeln!(
        output,
        "pub const WRAPPER_IMAGE_SIZE: u32 = {:#x};",
        rva(&wrapper["imageSize"])
    )
    .unwrap();
    hook(&mut output, "VERIFIER", &profile["native"]["verifier"]);
    writeln!(
        output,
        "pub const VERIFIER_RETURN_VALUE: u32 = {};",
        number(&profile["native"]["verifier"]["returnValue"])
    )
    .unwrap();
    for (name, key) in [
        ("APPEND_TASK", "appendTask"),
        ("GROW_TASKS", "growTasks"),
        ("EXECUTE_PROGRAM", "executeProgram"),
    ] {
        hook(&mut output, name, &wrapper["hooks"][key]);
    }
    writeln!(output, "pub const INSPECTION_PROFILE: InspectionProfile = InspectionProfile {{ time_date_stamp: WRAPPER_TIMESTAMP, image_size: WRAPPER_IMAGE_SIZE, patches: &[").unwrap();
    let mut ranges = Vec::new();
    for patch in wrapper["inspectionPatches"].as_array().unwrap() {
        let start = rva(&patch["rva"]);
        let expected = signature(&patch["expected"]);
        let replacement = signature(&patch["replacement"]);
        assert!(!expected.is_empty(), "empty image patch");
        assert_eq!(
            expected.len(),
            replacement.len(),
            "image patch length mismatch"
        );
        let end = start
            .checked_add(expected.len())
            .expect("image patch range overflow");
        assert!(
            end <= rva(&wrapper["imageSize"]),
            "image patch outside image"
        );
        assert!(
            ranges
                .iter()
                .all(|&(left, right)| end <= left || start >= right),
            "overlapping image patches"
        );
        ranges.push((start, end));
        writeln!(output, "ImagePatch {{ rva: {start:#x}, expected: &{expected:?}, replacement: &{replacement:?} }},").unwrap();
    }
    assert!(!ranges.is_empty(), "missing inspection patches");
    writeln!(output, "] }};").unwrap();
    let task = &wrapper["taskLayout"];
    let string = &task["string"];
    writeln!(output, "pub const TASK_LAYOUT: TaskLayout = TaskLayout {{ kind_offset: {}, program_offset: {}, program_types: &{:?}, string: ForeignStringLayout {{ bytes: {}, long_flag_mask: {}, long_length_offset: {}, long_data_offset: {}, short_data_offset: {}, short_length_shift: {} }} }};",
        number(&task["kindOffset"]), number(&task["programOffset"]),
        task["programTypes"].as_array().unwrap().iter().map(number).collect::<Vec<_>>(),
        number(&string["bytes"]), number(&string["longFlagMask"]), number(&string["longLengthOffset"]),
        number(&string["longDataOffset"]), number(&string["shortDataOffset"]), number(&string["shortLengthShift"])).unwrap();
    let directory = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(directory.join("profile.rs"), output).unwrap();

    let capture = &profile["native"]["capture"];
    let mut incident = String::new();
    for (name, key) in [
        ("CAPTURED", "captured"),
        ("DUMP_FAILED", "dumpFailed"),
        ("ARM_FAILED", "armFailed"),
    ] {
        writeln!(
            incident,
            "pub const {name}: u32 = {};",
            number(&capture["exitCodes"][key])
        )
        .unwrap();
    }
    for (name, key) in [
        ("STATE_UNINITIALIZED", "uninitialized"),
        ("STATE_WAITING", "waitingForWrapper"),
        ("STATE_ARMED", "armed"),
        ("STATE_FAILED", "failed"),
    ] {
        writeln!(
            incident,
            "pub const {name}: u32 = {};",
            number(&capture["states"][key])
        )
        .unwrap();
    }
    writeln!(
        incident,
        "pub const MAX_CAPTURE_DIRECTORY_UNITS: usize = {};",
        number(&capture["maxDirectoryCodeUnits"])
    )
    .unwrap();
    fs::write(directory.join("incident.rs"), incident).unwrap();
}
