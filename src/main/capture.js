// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const { PhantomLoader } = require("./api.js");
const fs = require("node:fs");
const path = require("node:path");
const { createHash } = require("node:crypto");
const profile = require("../../distribution/profile.json");
const wrapper = profile.files.find(file => file.path.endsWith("/resources/app/wrapper.node"));
const { exitCodes, states } = profile.native.capture;

let armed = false;

exports.armCapture = () => {
    if (armed) return;
    try {
        const native = PhantomLoader.api.native;
        const wrapperPath = path.join(process.resourcesPath, "app", "wrapper.node");
        const expectedSize = wrapper.bytes;
        const expectedSha256 = wrapper.sha256;
        const size = fs.statSync(wrapperPath).size;
        if (size !== expectedSize) {
            throw new Error(`Unsupported wrapper.node size: expected ${expectedSize}, got ${size} (${wrapperPath}).`);
        }
        const sha256 = createHash("sha256").update(fs.readFileSync(wrapperPath)).digest("hex");
        if (sha256 !== expectedSha256) {
            throw new Error(`Unsupported wrapper.node SHA-256: expected ${expectedSha256}, got ${sha256} (${wrapperPath}).`);
        }

        const outputDirectory = path.join(PhantomLoader.path.data, "dlvm_capture", "captures");
        fs.mkdirSync(outputDirectory, { recursive: true });
        const status = native.armDlVmCapture(outputDirectory);
        armed = true;
        console.log(`[PhantomLoader] DLVM capture ${status === states.waitingForWrapper ? "waiting for wrapper.node" : "armed"}; output: ${outputDirectory}`);
    } catch (error) {
        console.error(`[PhantomLoader] DLVM capture startup failed; terminating QQNT with exit code ${exitCodes.armFailed}.`, error);
        process.exit(exitCodes.armFailed);
    }
};

exports.getStatus = () => PhantomLoader.api.native.dlVmCaptureStatus();
