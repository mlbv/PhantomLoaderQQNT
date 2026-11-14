// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

exports.Runtime = class {

    static #plugins = new Map();

    static registerPlugin(plugin, exports, api) {
        exports.onLoad?.(api);
        this.#plugins.set(plugin, exports);
    }

    static triggerHooks(name, args) {
        for (const [plugin, exports] of this.#plugins) {
            try { exports[name]?.(...(typeof args == "function" ? args(plugin) : args)); }
            catch (error) { console.log(`[Main] [${plugin.manifest.slug}] [${name}]: `, error); }
        }
    }

}