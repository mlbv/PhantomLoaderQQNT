// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const fs = require("node:fs");
const path = require("node:path");

const entryKinds = ["main", "preload", "renderer"];
const isString = value => typeof value === "string";
const isObject = value => value !== null && typeof value === "object" && !Array.isArray(value);

function isPluginSlug(value) {
    return isString(value) && value.length > 0 && !/[<>:"/\\|?*\x00-\x1f]|[. ]$/.test(value) &&
        !/^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)/i.test(value);
}

function isWithin(directory, file) {
    const relative = path.relative(directory, file);
    return relative !== "" && relative !== ".." && !relative.startsWith(`..${path.sep}`) &&
        !path.isAbsolute(relative);
}

function validateEntry(directory, entry, kind) {
    const candidate = path.resolve(directory, entry);
    if (path.isAbsolute(entry) || !isWithin(directory, candidate)) {
        throw new Error(`Plugin ${kind} entry must be inside the plugin directory: ${entry}`);
    }
    const file = kind === "main" ? require.resolve(candidate) :
        (kind === "preload" ? ["", ".js", ".cjs", ".json"] : [""])
            .map(ext => candidate + ext)
            .find(file => fs.statSync(file, { throwIfNoEntry: false })?.isFile());
    if (!file || !isWithin(fs.realpathSync(directory), fs.realpathSync(file))) {
        throw new Error(`Plugin ${kind} entry is missing or outside the plugin directory: ${entry}`);
    }
}

function readPluginManifest(directory, { staged = false } = {}) {
    const manifest = JSON.parse(fs.readFileSync(path.join(directory, "manifest.json"), "utf-8"));
    if (!isObject(manifest) || !isPluginSlug(manifest.slug)) throw new Error("Invalid plugin slug");
    const api = manifest.api ?? "LiteLoader";
    if (api !== "LiteLoader" && api !== "PhantomLoader") {
        throw new Error(`Unknown plugin API: ${api} (${manifest.slug})`);
    }
    if (!Array.isArray(manifest.platform) || !manifest.platform.every(isString)) {
        throw new Error("Plugin manifest must declare a platform array");
    }
    if (manifest.dependencies != null &&
        (!Array.isArray(manifest.dependencies) || !manifest.dependencies.every(isPluginSlug))) {
        throw new Error("Plugin dependencies must be an array of slugs");
    }
    if (manifest.authors != null && (!Array.isArray(manifest.authors) ||
        !manifest.authors.every(author => isObject(author) && isString(author.name) && isString(author.link)))) {
        throw new Error("Plugin authors must be an array of names and links");
    }
    if (manifest.injects != null && !isObject(manifest.injects)) throw new Error("Invalid plugin entries");

    if (staged && (manifest.manifest_version !== 4 || !/^[a-zA-Z0-9_-]+$/.test(manifest.slug) ||
        ![manifest.name, manifest.description, manifest.version].every(isString) || !manifest.authors)) {
        throw new Error("Plugin package must contain a valid manifest v4");
    }

    for (const kind of entryKinds) {
        const entry = manifest.injects?.[kind];
        if (entry == null || entry === "") continue;
        if (!isString(entry)) throw new Error(`Plugin ${kind} entry must be a path`);
        if (staged) validateEntry(directory, entry, kind);
    }
    return manifest;
}

module.exports = { isPluginSlug, isWithin, readPluginManifest };
