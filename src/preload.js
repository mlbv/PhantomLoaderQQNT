// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const { contextBridge } = require("electron");
const { Runtime } = require("./preload/runtime.js");

for (const plugin of Object.values(PhantomLoader.plugins)) {
    if (plugin.disabled || plugin.incompatible || !plugin.path.injects.preload) continue;
    try {
        Runtime.registerPlugin(plugin, require(plugin.path.injects.preload),
            plugin.manifest.api === "PhantomLoader" ? PhantomLoader : LiteLoader);
    }
    catch (error) { console.log(`[Preload] [${plugin.manifest.slug}]: `, error); }
}

contextBridge.executeInMainWorld({
    func: async api => {
        const renderer = await import("local://root/src/renderer.js");
        await renderer.initialize(api);
    },
    args: [PhantomLoader]
});