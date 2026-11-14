// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const { PhantomLoader, LiteLoader } = require("./main/api.js");
const store = require("./main/store.js");
const { installHook } = require("./main/hook.js");
const { loadAllPlugins } = require("./main/loader.js");
const { Runtime } = require("./main/runtime.js");
const { armCapture } = require("./main/capture.js");

installHook();

const config = PhantomLoader.api.config.get("PhantomLoader", require("./common/static/config.json"));
if (config.enable_dlvm_capture !== false) armCapture();
for (const slug in config.deleting_plugins) store.deletePlugin(slug, [true, true], true);
for (const slug in config.installing_plugins) store.installPlugin(slug);
loadAllPlugins();
for (const plugin of Object.values(PhantomLoader.plugins)) {
    if (plugin.disabled || plugin.incompatible || !plugin.path.injects.main) continue;
    try {
        Runtime.registerPlugin(plugin, require(plugin.path.injects.main),
            plugin.manifest.api === "PhantomLoader" ? PhantomLoader : LiteLoader);
    }
    catch (error) { console.log(`[Main] [${plugin.manifest.slug}]: `, error); }
}

if (!globalThis.qwqnt) {
    const main_path = "./application.asar/app_launcher/index.js";
    require(require("path").join(process.resourcesPath, "app", main_path));
    setImmediate(() => global.launcher.installPathPkgJson.main = main_path);
}