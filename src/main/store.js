// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const PhantomLoader = require("./state.js");
const path = require("path");
const fs = require("fs");
const electron = require("electron");
const { randomUUID } = require("node:crypto");
const { isPluginSlug, readPluginManifest } = require("./plugin-manifest.js");
const default_config = require("../common/static/config.json");

const AdmZip = (() => {
    const major_node = path.join(process.resourcesPath, "./app/major.node");
    require(major_node).load("internal_admzip", module);
    return module.exports.admZip.default;
})();

function showErrorDialog(title, message) {
    const showDialog = () => electron.dialog.showMessageBox(null, {
        type: "error",
        title: "PhantomLoaderQQNT",
        message: `${title}\n${message}`
    });
    electron.app.isReady() ? showDialog() : electron.app.once("ready", showDialog);
}

const stagingRoot = () => path.join(PhantomLoader.path.plugins, ".pending-installations");

function pendingPaths(slug, record) {
    if (!isPluginSlug(slug) || !/^[a-zA-Z0-9_-]+$/.test(slug) ||
        typeof record?.stage !== "string" || !/^install-[a-zA-Z0-9]+$/.test(record.stage) ||
        typeof record.canonical !== "boolean" ||
        (record.previous != null && !isPluginSlug(record.previous))) {
        throw new Error("Invalid pending plugin installation");
    }
    const directory = record.canonical ? slug : `${slug}_${record.stage.slice("install-".length)}`;
    return {
        staged: path.join(stagingRoot(), record.stage),
        destination: path.join(PhantomLoader.path.plugins, directory),
        previous: record.previous == null ? undefined : path.join(PhantomLoader.path.plugins, record.previous)
    };
}

function removeStaging(directory) {
    try { fs.rmSync(directory, { recursive: true, force: true }); }
    catch (error) { console.warn(`[Plugins] Could not remove staging directory ${directory}`, error); }
}

exports.installPlugin = (plugin_path) => {
    let staged;
    try {
        let copy;
        if (path.extname(plugin_path).toLowerCase() === ".zip") {
            const zip = new AdmZip(plugin_path);
            copy = destination => zip.extractAllTo(destination);
        } else if (path.basename(plugin_path).toLowerCase() === "manifest.json") {
            copy = destination => fs.cpSync(path.dirname(plugin_path), destination, { recursive: true });
        } else {
            return false;
        }
        fs.mkdirSync(stagingRoot(), { recursive: true });
        staged = fs.mkdtempSync(path.join(stagingRoot(), "install-"));
        copy(staged);
        const { slug } = readPluginManifest(staged, { staged: true });
        const config = PhantomLoader.api.config.get("PhantomLoader", default_config);
        const pending = Object.assign(Object.create(null), config.pending_plugins);
        const superseded = pending[slug];
        const current = PhantomLoader.plugins[slug]?.path?.plugin ??
            exports.scanPluginDirectory().findLast(plugin => plugin.manifest.slug === slug)?.path;
        const destination = path.join(PhantomLoader.path.plugins, slug);
        const canonical = !fs.existsSync(destination);
        if (!current && !superseded && canonical) {
            fs.renameSync(staged, destination);
            return true;
        }
        pending[slug] = {
            stage: path.basename(staged),
            canonical,
            previous: current == null ? undefined : path.basename(current)
        };
        const deleting = { ...config.deleting_plugins };
        delete deleting[slug];
        if (!PhantomLoader.api.config.set("PhantomLoader", { ...config, pending_plugins: pending, deleting_plugins: deleting })) {
            throw new Error("无法保存插件变更，请检查配置目录的写入权限");
        }
        staged = undefined;
        if (superseded) {
            try { removeStaging(pendingPaths(slug, superseded).staged); }
            catch (error) { console.warn(`[Plugins] Invalid superseded installation for ${slug}`, error); }
        }
        return true;
    }
    catch (error) {
        if (staged) removeStaging(staged);
        showErrorDialog("安装插件时报错，请检查并手动安装", error.message);
        return false;
    }
};

exports.applyPendingPlugins = () => {
    const config = PhantomLoader.api.config.get("PhantomLoader", default_config);
    const pending = { ...config.pending_plugins };
    const activated = new Map();
    let changed = false;

    for (const [slug, record] of Object.entries(pending)) {
        try {
            const { staged, destination, previous } = pendingPaths(slug, record);
            const needsMove = fs.existsSync(staged);
            const manifest = readPluginManifest(needsMove ? staged : destination, { staged: true });
            if (manifest.slug !== slug) throw new Error("Pending plugin slug does not match its manifest");
            if (needsMove) {
                if (fs.existsSync(destination)) throw new Error(`Plugin destination already exists: ${destination}`);
                fs.renameSync(staged, destination);
            }
            activated.set(slug, destination);
            const retired = path.join(stagingRoot(), `retired-${record.stage}`);
            if (previous && path.relative(destination, previous) && fs.existsSync(previous)) {
                if (readPluginManifest(previous).slug !== slug) {
                    throw new Error(`Previous plugin directory changed: ${previous}`);
                }
                fs.renameSync(previous, retired);
            }
            removeStaging(retired);
            delete pending[slug];
            changed = true;
        } catch (error) {
            console.warn(`[Plugins] Could not apply pending installation for ${slug}`, error);
        }
    }
    if (changed && !PhantomLoader.api.config.set("PhantomLoader", { ...config, pending_plugins: pending })) {
        console.warn("[Plugins] Could not save pending installations");
    }
    return activated;
};

exports.deletePlugin = (slug, [self, data], now) => {
    const config = PhantomLoader.api.config.get("PhantomLoader", default_config);
    const plugin = config.deleting_plugins[slug];
    if (now) {
        if (self && plugin.self) fs.rmSync(plugin.self, { recursive: true, force: true });
        if (data && plugin.data) fs.rmSync(plugin.data, { recursive: true, force: true });
        delete config.deleting_plugins[slug];
    } else {
        config.deleting_plugins[slug] = {
            self: self ? PhantomLoader.plugins[slug]?.path?.plugin ?? null : null,
            data: data ? PhantomLoader.plugins[slug]?.path?.data ?? null : null
        };
        if (!(self || data)) delete config.deleting_plugins[slug];
    }
    if (!PhantomLoader.api.config.set("PhantomLoader", config)) {
        throw new Error("无法保存插件变更，请检查配置目录的写入权限");
    }
}

exports.disablePlugin = (slug, disabled) => {
    const config = PhantomLoader.api.config.get("PhantomLoader", default_config);
    config.disabled_plugins = disabled ?
        config.disabled_plugins.filter(item => item != slug) :
        config.disabled_plugins.concat(slug);
    PhantomLoader.api.config.set("PhantomLoader", config);
}

exports.setPluginConfig = (slug, config) => {
    let temporary;
    try {
        const configPath = path.join(PhantomLoader.path.data, slug === "PhantomLoader" ? "LiteLoader" : slug, "config.json");
        const directory = path.dirname(configPath);
        const serialized = JSON.stringify(config, null, 4);
        fs.mkdirSync(directory, { recursive: true });
        temporary = path.join(directory, `.config-${randomUUID()}.tmp`);
        fs.writeFileSync(temporary, serialized, { encoding: "utf-8", flag: "wx" });
        fs.renameSync(temporary, configPath);
        return true;
    } catch {
        if (temporary) {
            try { fs.rmSync(temporary, { force: true }); }
            catch (error) { console.warn(`[Plugins] Could not remove temporary configuration ${temporary}`, error); }
        }
        return false;
    }
}

exports.getPluginConfig = (slug, config) => {
    try {
        const config_path = path.join(PhantomLoader.path.data, slug === "PhantomLoader" ? "LiteLoader" : slug, "config.json");
        if (fs.existsSync(config_path)) {
            const content = fs.readFileSync(config_path, "utf-8");
            return Object.assign({}, config, JSON.parse(content));
        }
        else {
            exports.setPluginConfig(slug, config);
            return Object.assign({}, config, {});
        }
    }
    catch { return config; }
}

exports.scanPluginDirectory = () => {
    fs.mkdirSync(PhantomLoader.path.plugins, { recursive: true });
    const plugins = [];
    for (const directory of fs.readdirSync(PhantomLoader.path.plugins, { withFileTypes: true })) {
        if ((!directory.isDirectory() && !directory.isSymbolicLink()) ||
            directory.name === ".pending-installations" || directory.name.startsWith(".install-")) continue;
        const pluginPath = path.join(PhantomLoader.path.plugins, directory.name);
        if (!fs.existsSync(path.join(pluginPath, "manifest.json"))) continue;
        try { plugins.push({ path: pluginPath, manifest: readPluginManifest(pluginPath) }); }
        catch (error) { console.warn(`[Plugins] Could not read manifest in ${pluginPath}`, error); }
    }
    return plugins;
}
