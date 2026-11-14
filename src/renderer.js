// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

import { initialize as initializeApi } from "./renderer/api.js";
import { Runtime } from "./renderer/runtime.js";

export async function initialize(api) {
    initializeApi(api);
    await import("./renderer/components/renderer.js");
    await import("./renderer/triggers/renderer.js");
    const { installHook } = await import("./renderer/hook.js");
    installHook();

    for (const plugin of Object.values(api.plugins)) {
        if (plugin.disabled || plugin.incompatible || !plugin.path.injects.renderer) continue;
        try {
            const exports = await import(`local:///${plugin.path.injects.renderer}`);
            Runtime.registerPlugin(plugin, exports,
                plugin.manifest.api === "PhantomLoader" ? api : LiteLoader);
        }
        catch (error) { console.log(`[Renderer] [${plugin.manifest.slug}]: `, error); }
    }
}
