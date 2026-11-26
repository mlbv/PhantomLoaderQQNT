// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const path = require("path");
const electron = require("electron");
const { interception } = require("./interception.js");

const PhantomLoader = exports.PhantomLoader = require("./state.js");
const LiteLoader = exports.LiteLoader = Object.create(null);
const store = require("./store.js");

PhantomLoader.plugins = Object.create(null);

PhantomLoader.path = Object.create(null);
PhantomLoader.path.root = path.dirname(path.dirname(__dirname));
PhantomLoader.path.profile = path.resolve(
    process.env.PHANTOMLOADERQQNT_PROFILE ?? process.env.LITELOADERQQNT_PROFILE ??
    (globalThis.qwqnt ? path.join(globalThis.qwqnt.framework.paths.data, "LiteLoader") : PhantomLoader.path.root)
);
PhantomLoader.path.data = path.join(PhantomLoader.path.profile, "data");
PhantomLoader.path.plugins = path.join(PhantomLoader.path.profile, "plugins");

PhantomLoader.package = Object.create(null);
PhantomLoader.package.phantomloader = require(path.join(PhantomLoader.path.root, "package.json"));
PhantomLoader.package.liteloader = require("../common/static/liteloader.json");
PhantomLoader.package.qqnt = require(path.join(process.resourcesPath, "app", "package.json"));

PhantomLoader.versions = Object.create(null);
PhantomLoader.versions.phantomloader = PhantomLoader.package.phantomloader.version;
PhantomLoader.versions.liteloader = PhantomLoader.package.liteloader.version;
PhantomLoader.versions.qqnt = PhantomLoader.package.qqnt.version;
PhantomLoader.versions.electron = process.versions.electron;
PhantomLoader.versions.node = process.versions.node;
PhantomLoader.versions.chrome = process.versions.chrome;

PhantomLoader.os = Object.create(null);
PhantomLoader.os.platform = process.platform;

PhantomLoader.api = Object.create(null);
PhantomLoader.api.openExternal = (url) => (electron.shell.openExternal(url), true);
PhantomLoader.api.openPath = (path) => (electron.shell.openPath(path), true);

PhantomLoader.api.intercept = Object.create(null);
PhantomLoader.api.intercept.register = interception.register.bind(interception);
PhantomLoader.api.intercept.disposeOwner = interception.disposeOwner.bind(interception);

let native;
Object.defineProperty(PhantomLoader.api, "native", {
    get: () => native ??= process._linkedBinding("phantomloader_native")
});

PhantomLoader.api.config = Object.create(null);
PhantomLoader.api.config.set = store.setPluginConfig;
PhantomLoader.api.config.get = store.getPluginConfig;

PhantomLoader.api.plugin = Object.create(null);
PhantomLoader.api.plugin.install = store.installPlugin;
PhantomLoader.api.plugin.delete = store.deletePlugin;
PhantomLoader.api.plugin.disable = store.disablePlugin;

Object.defineProperty(LiteLoader, "plugins", {
    enumerable: true,
    get: () => PhantomLoader.plugins,
    set: value => { PhantomLoader.plugins = value; }
});
LiteLoader.path = PhantomLoader.path;
LiteLoader.package = Object.assign(Object.create(null), {
    liteloader: PhantomLoader.package.liteloader,
    qqnt: PhantomLoader.package.qqnt
});
LiteLoader.versions = Object.assign(Object.create(null),
    Object.fromEntries(Object.entries(PhantomLoader.versions).filter(([name]) => name !== "phantomloader")));
LiteLoader.os = PhantomLoader.os;
LiteLoader.api = Object.assign(Object.create(null), {
    openExternal: PhantomLoader.api.openExternal,
    openPath: PhantomLoader.api.openPath,
    config: PhantomLoader.api.config,
    plugin: PhantomLoader.api.plugin
});

Object.defineProperty(globalThis, "LiteLoader", {
    get() {
        const stack = new Error().stack.split("\n")[2];
        const allowed = [PhantomLoader.path.root, PhantomLoader.path.profile];
        return allowed.some(item => stack.includes(item)) ? LiteLoader : null;
    }
});

const rendererOperations = new Map([
    ["api.config.get", store.getPluginConfig],
    ["api.config.set", store.setPluginConfig],
    ["api.plugin.install", store.installPlugin],
    ["api.plugin.delete", store.deletePlugin],
    ["api.plugin.disable", store.disablePlugin],
    ["api.openExternal", PhantomLoader.api.openExternal],
    ["api.openPath", PhantomLoader.api.openPath]
]);

const handleApi = api => (event, method, args) => {
    event.returnValue = method.length ?
        rendererOperations.get(method.join("."))?.(...args) ?? null :
        JSON.parse(JSON.stringify(api));
};
electron.ipcMain.on("PhantomLoader.PhantomLoader.PhantomLoader", handleApi(PhantomLoader));
electron.ipcMain.on("LiteLoader.LiteLoader.LiteLoader", handleApi(LiteLoader));

electron.protocol.registerSchemesAsPrivileged([
    {
        scheme: "local",
        privileges: {
            standard: false,
            allowServiceWorkers: true,
            corsEnabled: false,
            supportFetchAPI: true,
            stream: true,
            bypassCSP: true
        }
    }
]);

electron.app.on("ready", () => {
    const schemes = ["local"];
    const old_schemes = electron.app.commandLine.getSwitchValue("fetch-schemes");
    const new_schemes = [old_schemes, ...schemes].join(",");
    electron.app.commandLine.appendSwitch("fetch-schemes", new_schemes);
});

exports.protocolRegister = (protocol) => {
    if (!protocol.isProtocolRegistered("local")) {
        protocol.handle("local", (req) => {
            const { host, pathname } = new URL(decodeURI(req.url));
            const filepath = path.normalize(pathname.slice(1));
            switch (host) {
                case "root": return electron.net.fetch(`file:///${PhantomLoader.path.root}/${filepath}`);
                case "profile": return electron.net.fetch(`file:///${PhantomLoader.path.profile}/${filepath}`);
                default: return electron.net.fetch(`file://${host}/${filepath}`);
            }
        });
    }
}
