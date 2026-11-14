// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const path = require("path");
const { PhantomLoader, protocolRegister } = require("./api.js");
const { Runtime } = require("./runtime.js");
const { interception } = require("./interception.js");
const { installTransport } = require("./transport.js");

const hookKey = Symbol.for("LiteLoader.main.hook");
const preloadContents = new WeakSet();
const preloadSessions = new WeakSet();


function proxyPreload(func) {
    if (func?.name == "_getPreloadPaths") return new Proxy(func, {
        apply(target, thisArg, argArray) {
            return [
                path.join(PhantomLoader.path.root, "./src/preload/api.js"),
                path.join(PhantomLoader.path.root, "./src/preload/module.js"),
                ...Reflect.apply(target, thisArg, argArray)
            ];
        }
    });
    if (func?.name == "getPreloadScripts") return new Proxy(func, {
        apply(target, thisArg, argArray) {
            return [
                { filePath: path.join(PhantomLoader.path.root, "./src/preload/api.js"), type: "frame" },
                { filePath: path.join(PhantomLoader.path.root, "./src/preload/module.js"), type: "frame" },
                ...Reflect.apply(target, thisArg, argArray)
            ];
        }
    });
}


function proxyWindow(func, transport) {
    return new Proxy(func, {
        construct(target, argArray, newTarget) {
            Runtime.triggerHooks("onBrowserWindowCreating", [target, argArray, newTarget]);
            const window = Reflect.construct(target, argArray, newTarget);
            protocolRegister(window.webContents.session.protocol);
            transport.attach(window.webContents, window);
            if (!preloadContents.has(window.webContents)) {
                window.webContents._getPreloadPaths = proxyPreload(window.webContents._getPreloadPaths);
                preloadContents.add(window.webContents);
            }
            if (!preloadSessions.has(window.webContents.session)) {
                window.webContents.session.getPreloadScripts = proxyPreload(window.webContents.session.getPreloadScripts);
                preloadSessions.add(window.webContents.session);
            }
            Runtime.triggerHooks("onBrowserWindowCreated", [window]);
            return window;
        }
    });
}


function proxyElectron(func, transport) {
    const constructors = new WeakMap();
    return new Proxy(func, {
        get(target, property, receiver) {
            const module = Reflect.get(target, property, receiver);
            return property != "exports" ? module : new Proxy(module, {
                get(target, property, receiver) {
                    const exports = Reflect.get(target, property, receiver);
                    if (property != "BrowserWindow") return exports;
                    if (!constructors.has(exports)) constructors.set(exports, proxyWindow(exports, transport));
                    return constructors.get(exports);
                }
            });
        }
    })
}


exports.installHook = () => {
    const electron = require("electron");
    if (electron.app[hookKey]) return;
    const transport = installTransport(electron, interception, context => {
        if (!context.channel.includes("RM_IPCFROM_")) return;
        for (const command of context.commands) {
            if (command.name == "nodeIKernelSessionListener/onSessionInitComplete") {
                Runtime.triggerHooks("onLogin", [command.payload.uid]);
            }
        }
    });
    require.cache["electron"] = proxyElectron(require.cache["electron"], transport);
    Object.defineProperty(electron.app, hookKey, { value: true });
}