// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const { ipcRenderer, contextBridge } = require("electron");

function createApi(name) {
    const sendSync = (method, args) => ipcRenderer.sendSync([name, name, name].join("."), method, args);
    return {
        ...sendSync([], []),
        api: {
            config: {
                get: async (...args) => sendSync(["api", "config", "get"], args),
                set: async (...args) => sendSync(["api", "config", "set"], args)
            },
            plugin: {
                install: async (...args) => sendSync(["api", "plugin", "install"], args),
                delete: async (...args) => sendSync(["api", "plugin", "delete"], args),
                disable: async (...args) => sendSync(["api", "plugin", "disable"], args)
            },
            openExternal: async (...args) => sendSync(["api", "openExternal"], args),
            openPath: async (...args) => sendSync(["api", "openPath"], args)
        }
    };
}

module.exports.PhantomLoader = createApi("PhantomLoader");
module.exports.LiteLoader = createApi("LiteLoader");
if (!contextBridge.executeInMainWorld({ func: () => "LiteLoader" in globalThis })) {
    contextBridge.exposeInMainWorld("LiteLoader", module.exports.LiteLoader);
}
