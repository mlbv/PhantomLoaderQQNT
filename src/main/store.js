// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const PhantomLoader = require("./state.js");
const path = require("path");
const fs = require("fs");
const electron = require("electron");
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

exports.installPlugin = (plugin_path) => {
    let staged_path;
    let installed_path;
    try {
        let manifest_text;
        let copy;
        if (path.extname(plugin_path).toLowerCase() == ".zip") {
            const plugin_zip = new AdmZip(plugin_path);
            manifest_text = plugin_zip.readAsText(plugin_zip.getEntry("manifest.json"));
            copy = destination => plugin_zip.extractAllTo(destination);
        } else if (path.basename(plugin_path).toLowerCase() == "manifest.json") {
            manifest_text = fs.readFileSync(plugin_path, "utf-8");
            copy = destination => fs.cpSync(path.dirname(plugin_path), destination, { recursive: true });
        } else {
            return false;
        }
        const { slug } = JSON.parse(manifest_text);
        if (typeof slug !== "string" || !/^[a-zA-Z0-9_-]+$/.test(slug)) {
            throw new Error("插件 slug 必须是目录名称，只能包含字母、数字、下划线或连字符");
        }
        fs.mkdirSync(PhantomLoader.path.plugins, { recursive: true });
        staged_path = fs.mkdtempSync(path.join(PhantomLoader.path.plugins, ".install-"));
        copy(staged_path);
        let destination = path.join(PhantomLoader.path.plugins, slug);
        if (fs.existsSync(destination)) destination += "_" + path.basename(staged_path).slice(".install-".length);
        fs.renameSync(staged_path, destination);
        installed_path = destination;
        staged_path = undefined;
        if (slug in PhantomLoader.plugins) exports.deletePlugin(slug, [true, false], false);
        return true;
    }
    catch (err) {
        if (staged_path) fs.rmSync(staged_path, { recursive: true, force: true });
        if (installed_path) fs.rmSync(installed_path, { recursive: true, force: true });
        showErrorDialog("安装插件时报错，请检查并手动安装", err.message);
        return false;
    }
}

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
    try {
        const config_path = path.join(PhantomLoader.path.data, slug === "PhantomLoader" ? "LiteLoader" : slug, "config.json");
        fs.mkdirSync(path.dirname(config_path), { recursive: true });
        fs.writeFileSync(config_path, JSON.stringify(config, null, 4), "utf-8");
        return true;
    }
    catch { return false; }
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
    return fs.readdirSync(PhantomLoader.path.plugins, { withFileTypes: true })
        .map(dirent => path.join(PhantomLoader.path.plugins, dirent.name, "manifest.json"))
        .filter(manifest => fs.existsSync(manifest))
        .map(manifest => ({
            path: path.dirname(manifest),
            manifest: JSON.parse(fs.readFileSync(manifest, "utf-8"))
        }));
}
