// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const { ipcRenderer, contextBridge } = require("electron");

function createApi(name) {
    const sendSync = (method, args) => ipcRenderer.sendSync([name, name, name].join("."), method, args);const snapshot = sendSync([], []);
    if (!snapshot) return;
    return {
        ...snapshot,
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
if (module.exports.LiteLoader && !contextBridge.executeInMainWorld({ func: () => "LiteLoader" in globalThis })) {
    contextBridge.exposeInMainWorld("LiteLoader", module.exports.LiteLoader);
}

class File {
    static request(path, method) {
        const xhr = new XMLHttpRequest();
        xhr.open(method, `local:///${path}`, false);
        xhr.send();
        return xhr.status == 200 && (method === "HEAD" || xhr.responseText);
    }
    static read(path) { return File.request(path, "GET"); }
    static exists(path) { return !!File.request(path, "HEAD"); }
}

class Path {
    static isWindows(path) { return /^[a-zA-Z]:[\\/]/.test(path); }
    static isUnix(path) { return path.startsWith("/"); }
    static isAbsolute(path) { return Path.isUnix(path) || Path.isWindows(path); }
    static getDrive(path) { return Path.isWindows(path) ? `${path.slice(0, 2)}/` : Path.isUnix(path) ? "/" : ""; }
    static normalize(path) {
        const drive = Path.getDrive(path);
        const parts = path.slice(drive.length).split(/[\\/]/);
        const stack = [];
        for (const part of parts) if (part && part != ".") part == ".." ? stack.pop() : stack.push(part);
        return `${drive}${stack.join("/")}` || ".";
    }
    static dirname(path) {
        const parts = Path.normalize(path).split("/").slice(0, -1);
        return parts.length ? parts.join("/") : ".";
    }
    static extname(path) { return path.match(/\.[^/.]+$/)?.[0] ?? ""; }
}

class Module {
    static cache = new Map();

    constructor(id, parent = null) {
        this.id = id;
        this.exports = {};
        this.parent = parent;
    }

    require(id) {
        if (!Path.isAbsolute(id) && !id.startsWith(".")) {
            try { return require(id); }
            catch {}
        }

        const base = Path.dirname(this.id);
        const resolved = Path.normalize(Path.isAbsolute(id) ? id : `${base}/${id}`);
        for (const ext of ["", ".js", ".cjs", ".json"]) {
            const file = resolved + ext;
            if (Module.cache.has(file)) return Module.cache.get(file).exports;
            if (File.exists(file)) return Module.load(file, this);
        }
        throw new Error(`Module not found: ${id} (from ${base})`);
    }

    static load(file, parent) {
        const entry = new Module(file, parent);
        this.cache.set(file, entry);
        try {
            entry.#load(file);
            return entry.exports;
        } catch (error) {
            this.cache.delete(file);
            throw error;
        }
    }
    #load(file) {
        const context = {
            module: this, require: this.require.bind(this), exports: this.exports,
            global, process, Buffer, setImmediate, clearImmediate,
            __filename: file, __dirname: Path.dirname(file),
            LiteLoader: module.exports.LiteLoader
        };
        if (file.startsWith(Path.normalize(`${module.exports.PhantomLoader.path.root}/src`) + "/")) {
            context.PhantomLoader = module.exports.PhantomLoader;
        }
        switch (Path.extname(file)) {
            case ".js":
            case ".cjs": return new Function(...Object.keys(context), File.read(file))(...Object.values(context));
            case ".json": return this.exports = JSON.parse(File.read(file));
            default: throw new Error(`Unsupported module extension: ${file}`);
        }
    }
}

if (module.exports.PhantomLoader) {
    Module.load(Path.normalize(`${module.exports.PhantomLoader.path.root}/src/preload.js`), null);
}
