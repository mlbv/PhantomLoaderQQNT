#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const payloadExports = new Set([
    "PayloadMain", "RegisterNativeApi",
    "memcmp", "memcpy", "memmove", "memset", "strlen", "wcslen"
]);
const fixtureHelpers = [
    "InitializePayloadRuntime", "InitializeDlVmCapture",
    "InstallDlVmCaptureTargets", "DisposeNativeHooks"
];

function requireCondition(condition, message) {
    if (!condition) throw new Error(message);
}

function contains(length, offset, size) {
    return offset <= length && size <= length - offset;
}

function parsePe(bytes) {
    const range = (offset, size, label) => {
        requireCondition(contains(bytes.length, offset, size), `${label} exceeds file bounds`);
        return offset;
    };
    const u16 = (offset, label = "PE field") => bytes.readUInt16LE(range(offset, 2, label));
    const u32 = (offset, label = "PE field") => bytes.readUInt32LE(range(offset, 4, label));

    range(0, 64, "DOS header");
    requireCondition(u16(0) === 0x5a4d, "invalid DOS signature");
    const pe = u32(0x3c);
    requireCondition(pe >= 64, "invalid PE header offset");
    range(pe, 24, "PE header");
    requireCondition(u32(pe) === 0x4550, "invalid PE signature");
    requireCondition(u16(pe + 4) === 0x8664, "PE machine must be AMD64");
    const characteristics = u16(pe + 22);
    requireCondition((characteristics & 2) !== 0, "PE must be executable");
    requireCondition((characteristics & 1) === 0, "PE relocations must not be stripped");

    const optional = pe + 24;
    const optionalSize = u16(pe + 20);
    requireCondition(optionalSize >= 240, "truncated PE32+ optional header");
    range(optional, optionalSize, "optional header");
    requireCondition(u16(optional) === 0x20b, "optional header must be PE32+");
    const directoryCount = u32(optional + 108);
    requireCondition(directoryCount === 16 && 112 + directoryCount * 8 <= optionalSize,
        "optional header must contain the 16 PE data directories");
    const directories = Array.from({ length: directoryCount }, (_, index) => ({
        rva: u32(optional + 112 + index * 8),
        size: u32(optional + 116 + index * 8)
    }));
    const imageSize = u32(optional + 56);
    const headersSize = u32(optional + 60);
    requireCondition(headersSize !== 0 && headersSize <= bytes.length && imageSize >= headersSize,
        "invalid PE image/header sizes");
    const sectionCount = u16(pe + 6);
    const table = optional + optionalSize;
    requireCondition(sectionCount !== 0 && contains(headersSize, table, sectionCount * 40),
        "section table exceeds headers");
    range(table, sectionCount * 40, "section table");
    const sections = Array.from({ length: sectionCount }, (_, index) => {
        const offset = table + index * 40;
        const section = {
            rva: u32(offset + 12),
            virtualSize: u32(offset + 8),
            rawSize: u32(offset + 16),
            raw: u32(offset + 20),
            characteristics: u32(offset + 36)
        };
        section.size = Math.max(section.virtualSize, section.rawSize);
        requireCondition(section.rva >= headersSize && contains(imageSize, section.rva, section.size),
            `section ${index} exceeds image bounds`);
        if (section.rawSize !== 0) {
            requireCondition(section.raw >= headersSize && contains(bytes.length, section.raw, section.rawSize),
                `section ${index} exceeds raw file bounds`);
        }
        return section;
    });
    for (const [start, size, label] of [
        ["rva", "size", "virtual"], ["raw", "rawSize", "raw"]
    ]) {
        const ordered = sections.filter(section => section[size] !== 0)
            .toSorted((left, right) => left[start] - right[start]);
        for (let index = 1; index < ordered.length; index++) {
            requireCondition(ordered[index][start] >= ordered[index - 1][start] + ordered[index - 1][size],
                `overlapping ${label} sections`);
        }
    }

    const imageRange = (rva, size, label) => {
        requireCondition(contains(imageSize, rva, size) &&
            (contains(headersSize, rva, size) || sections.some(section =>
                rva >= section.rva && contains(section.size, rva - section.rva, size))),
        `${label} exceeds mapped section bounds`);
    };
    const fileRange = (rva, size, label) => {
        imageRange(rva, size, label);
        if (contains(headersSize, rva, size)) return range(rva, size, label);
        const section = sections.find(section => rva >= section.rva &&
            contains(section.rawSize, rva - section.rva, size));
        requireCondition(section !== undefined, `${label} is not backed by file bytes`);
        return range(section.raw + rva - section.rva, size, label);
    };
    const string = (rva, label) => {
        const start = fileRange(rva, 1, label);
        const section = sections.find(section => rva >= section.rva &&
            rva - section.rva < section.rawSize);
        const limit = section ? section.raw + section.rawSize : headersSize;
        const end = bytes.indexOf(0, start);
        requireCondition(end > start && end < limit, `${label} is empty or unterminated`);
        const value = bytes.subarray(start, end);
        requireCondition(value.every(byte => byte >= 0x20 && byte <= 0x7e), `${label} is not ASCII`);
        return value.toString("ascii");
    };
    const entry = u32(optional + 16);
    requireCondition(entry !== 0 && sections.some(section =>
        (section.characteristics & 0x20000000) !== 0 && entry >= section.rva &&
        entry - section.rva < section.rawSize), "entry point is outside executable section bytes");

    for (const [index, directory] of directories.entries()) {
        if (directory.rva === 0 && directory.size === 0) continue;
        const label = new Map([
            [1, "imports"], [9, "TLS"], [12, "import address table"], [13, "delay imports"]
        ]).get(index) || `data directory ${index}`;
        requireCondition(directory.rva !== 0 && directory.size !== 0, `incomplete ${label}`);
        if (index === 4) range(directory.rva, directory.size, "certificate directory");
        else imageRange(directory.rva, directory.size, label);
    }
    return { bytes, directories, characteristics, entry, sections, u16, u32, fileRange, imageRange, string };
}

function checkRelocations(pe) {
    const directory = pe.directories[5];
    requireCondition(directory.rva !== 0 && directory.size >= 8, "missing relocation directory");
    const start = pe.fileRange(directory.rva, directory.size, "relocation directory");
    let offset = start;
    const end = start + directory.size;
    let dir64 = 0;
    while (offset < end) {
        requireCondition(end - offset >= 8, "truncated relocation block");
        const page = pe.u32(offset);
        const size = pe.u32(offset + 4);
        requireCondition(size >= 8 && size <= end - offset && (size - 8) % 2 === 0,
            "invalid relocation block size");
        for (let item = offset + 8; item < offset + size; item += 2) {
            const entry = pe.u16(item);
            const type = entry >>> 12;
            if (type === 0) continue;
            requireCondition(type === 10, `unsupported relocation type ${type}; expected DIR64`);
            pe.imageRange(page + (entry & 0xfff), 8, "DIR64 relocation target");
            dir64++;
        }
        offset += size;
    }
    requireCondition(dir64 !== 0, "relocation directory has no DIR64 fixups");
    return dir64;
}

function checkExports(pe, isPayload) {
    const directory = pe.directories[0];
    if (directory.rva === 0) {
        requireCondition(!isPayload, "payload export directory is missing");
        return;
    }
    requireCondition(directory.size >= 40, "truncated export directory");
    const offset = pe.fileRange(directory.rva, directory.size, "export directory");
    const functionCount = pe.u32(offset + 20);
    const nameCount = pe.u32(offset + 24);
    requireCondition(functionCount === nameCount && nameCount !== 0,
        "exports must be named, without ordinal-only entries");
    const functions = pe.fileRange(pe.u32(offset + 28), functionCount * 4, "export address table");
    const names = pe.fileRange(pe.u32(offset + 32), nameCount * 4, "export name table");
    const ordinals = pe.fileRange(pe.u32(offset + 36), nameCount * 2, "export ordinal table");
    const seen = new Set();
    const seenOrdinals = new Set();
    for (let index = 0; index < nameCount; index++) {
        const name = pe.string(pe.u32(names + index * 4), "export name");
        requireCondition(isPayload && payloadExports.has(name), `unexpected export: ${name}`);
        requireCondition(!seen.has(name), `duplicate export: ${name}`);
        seen.add(name);
        const ordinal = pe.u16(ordinals + index * 2);
        requireCondition(ordinal < functionCount && !seenOrdinals.has(ordinal), "invalid export ordinal");
        seenOrdinals.add(ordinal);
        const target = pe.u32(functions + ordinal * 4);
        requireCondition(target < directory.rva || target >= directory.rva + directory.size,
            `forwarded export: ${name}`);
        requireCondition(pe.sections.some(section => (section.characteristics & 0x20000000) !== 0 &&
            target >= section.rva && target - section.rva < section.rawSize),
        `export ${name} is outside executable section bytes`);
        if (name === "PayloadMain") {
            requireCondition(target === pe.entry, "PayloadMain export differs from the PE entry point");
        }
    }
    if (isPayload) {
        for (const name of ["PayloadMain", "RegisterNativeApi"]) {
            requireCondition(seen.has(name), `missing payload export: ${name}`);
        }
    }
}

function checkStrings(bytes) {
    const texts = [
        bytes.toString("latin1").replaceAll("\\", "/"),
        bytes.toString("utf16le").replaceAll("\\", "/"),
        bytes.subarray(1).toString("utf16le").replaceAll("\\", "/")
    ];
    for (const text of texts) {
        for (const helper of fixtureHelpers) {
            requireCondition(!text.includes(helper), `fixture helper present: ${helper}`);
        }
    }
    const root = fileURLToPath(new URL("../", import.meta.url));
    const prefixes = [
        root, homedir(),
        process.env.CARGO_HOME || path.join(homedir(), ".cargo"),
        process.env.RUSTUP_HOME || path.join(homedir(), ".rustup")
    ].map(prefix => path.resolve(prefix).replaceAll("\\", "/").replace(/\/$/, "").toLowerCase());
    for (const text of texts) {
        const lower = text.toLowerCase();
        for (const prefix of prefixes) {
            requireCondition(!lower.includes(prefix + "/"), "unremapped private source path");
        }
        requireCondition(!/[a-z]:\/(?:[^/\x00-\x1f<>:"|?*]+\/)*[^/\x00-\x1f<>:"|?*]+\.(?:rs|pdb|cpp|c|h)(?:[\x00\s:]|$)/i.test(text),
            "unremapped absolute Windows source/debug path");
        requireCondition(!/\/(?:home|Users|root|tmp|private|opt|workspace|workspaces|builds|__w)\/(?:[^/\x00\s]+\/)*[^/\x00\s]+\.rs(?:[\x00\s:]|$)/.test(text),
            "unremapped absolute user source path");
    }
}

function checkNative(directory, filename, isPayload) {
    try {
        const bytes = readFileSync(path.join(directory, filename));
        const pe = parsePe(bytes);
        requireCondition(((pe.characteristics & 0x2000) !== 0) === isPayload,
            isPayload ? "payload must be a DLL" : "launcher must be an executable, not a DLL");
        if (isPayload) {
            for (const [index, label] of [[1, "imports"], [12, "import address table"], [13, "delay imports"], [9, "TLS"]]) {
                const entry = pe.directories[index];
                requireCondition(entry.rva === 0 && entry.size === 0, `payload must have no ${label}`);
            }
        }
        const relocations = checkRelocations(pe);
        checkExports(pe, isPayload);
        checkStrings(bytes);
        console.log(`${filename}: AMD64 PE32+, ${pe.sections.length} sections, ${relocations} DIR64 relocations; production checks passed`);
    } catch (error) {
        throw new Error(`${filename}: ${error.message}`);
    }
}

function main() {
    const args = process.argv.slice(2);
    if (args.length === 1 && ["--help", "-h"].includes(args[0])) {
        console.log("Usage: node distribution/check-native.mjs <native-release-directory>");
        return;
    }
    requireCondition(args.length === 1 && !args[0].startsWith("-"),
        "Usage: node distribution/check-native.mjs <native-release-directory>");
    const directory = path.resolve(args[0]);
    checkNative(directory, "phantomloader.exe", false);
    checkNative(directory, "phantomloader_payload.dll", true);
}

try {
    main();
} catch (error) {
    console.error(`Native verification failed: ${error.message}`);
    process.exitCode = 1;
}
