// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const { PhantomLoader } = require("./api.js");
const path = require("path");
const store = require("./store.js");
const config = PhantomLoader.api.config.get("PhantomLoader", require("../common/static/config.json"));

function topologicalSort(plugins) {
    const sorted = Object.create(null);
    const visited = new Set();
    const visiting = new Set();
    const visit = (slug) => {
        if (visited.has(slug)) return;
        if (visiting.has(slug)) return;
        const plugin = plugins[slug];
        if (!plugin) return;
        visiting.add(slug);
        (plugin.manifest.dependencies ?? []).forEach(visit);
        visiting.delete(slug);
        visited.add(slug);
        sorted[slug] = plugin;
    };
    Object.keys(plugins).forEach(visit);
    return sorted;
}

function getPluginInstance(plugin) {
    return {
        manifest: plugin.manifest,
        incompatible: !plugin.manifest.platform.includes(PhantomLoader.os.platform),
        disabled: config.disabled_plugins.includes(plugin.manifest.slug),
        path: {
            plugin: plugin.path,
            data: path.join(PhantomLoader.path.data, plugin.manifest.slug),
            injects: {
                main: plugin.manifest?.injects?.main ? path.join(plugin.path, plugin.manifest.injects.main) : null,
                preload: plugin.manifest?.injects?.preload ? path.join(plugin.path, plugin.manifest.injects.preload) : null,
                renderer: plugin.manifest?.injects?.renderer ? path.join(plugin.path, plugin.manifest.injects.renderer) : null
            }
        }
    };
}

exports.loadAllPlugins = (activated = new Map()) => {
    if (config.enable_plugins) {
        const plugins = Object.create(null);
        for (const plugin of store.scanPluginDirectory()) {
            const { slug } = plugin.manifest;
            if (activated.has(slug) && activated.get(slug) !== plugin.path) continue;
            plugins[slug] = getPluginInstance(plugin);
        }
        PhantomLoader.plugins = topologicalSort(plugins);
    }
}