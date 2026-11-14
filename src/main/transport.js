// SPDX-FileCopyrightText: 2023-2025 LiteLoaderQQNT
// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const transportKey = Symbol.for("LiteLoader.interception.transport");

function commandView(envelope, key, payloadKey = "payload") {
    return Object.defineProperties({}, {
        name: {
            enumerable: true,
            get: () => envelope[key],
            set: value => { envelope[key] = value; }
        },
        payload: {
            enumerable: true,
            get: () => envelope[payloadKey],
            set: value => { envelope[payloadKey] = value; }
        }
    });
}

function normalizeCommands(args) {
    const commands = [];
    const visit = (value, batch = true) => {
        if (Array.isArray(value)) {
            if (typeof value[0] === "string" && value.length >= 2) {
                commands.push(commandView(value, 0, 1));
            } else if (batch) {
                for (const envelope of value) visit(envelope, false);
            }
        } else if (value && typeof value === "object" && "payload" in value) {
            const key = typeof value.cmdName === "string" ? "cmdName" :
                typeof value.cmd === "string" ? "cmd" : undefined;
            if (key) commands.push(commandView(value, key));
        }
    };
    for (let index = 1; index < args.length; index++) visit(args[index]);
    return commands;
}

function createContext(args, webContents, window, ipc = true) {
    return {
        args,
        get channel() { return ipc ? this.args[0] : undefined; },
        webContents,
        window,
        get commands() { return ipc ? normalizeCommands(this.args) : []; }
    };
}

class Transport {
    constructor(electron, interception, onIncoming) {
        this.electron = electron;
        this.interception = interception;
        this.onIncoming = onIncoming;
        this.contents = new WeakMap();
        this.sessions = new WeakMap();

        const transport = this;
        electron.shell.openExternal = new Proxy(electron.shell.openExternal, {
            apply(target, thisArg, args) {
                const context = createContext(args, undefined, undefined, false);
                return interception.dispatch("external", context,
                    next => Reflect.apply(target, thisArg, next));
            }
        });
        electron.app.on("web-contents-created", (_event, contents) => transport.attach(contents));
        for (const contents of electron.webContents.getAllWebContents()) this.attach(contents);
    }

    attach(webContents, window) {
        const existing = this.contents.get(webContents);
        if (existing) {
            if (window) existing.window = window;
            return;
        }
        const state = { webContents, id: webContents.id, window, active: true };
        this.contents.set(webContents, state);
        const transport = this;
        const send = webContents.send;
        state.send = send;
        state.sendProxy = new Proxy(send, {
            apply(target, thisArg, args) {
                if (!state.active) return Reflect.apply(target, thisArg, args);
                const context = transport.context(state, args);
                transport.onIncoming?.(context);
                return transport.interception.dispatch("incoming", context,
                    next => state.active ? Reflect.apply(target, thisArg, next) : undefined);
            }
        });
        webContents.send = state.sendProxy;

        // Older Electron routes native IPC through WebContents; current versions use Session.
        state.emit = webContents.emit;
        state.emitProxy = this.wrapEmitter(state.emit, () => state);
        webContents.emit = state.emitProxy;
        const session = webContents.session;
        let sessionState = this.sessions.get(session);
        if (!sessionState) {
            sessionState = { contents: new Map() };
            sessionState.proxy = this.wrapEmitter(session.emit,
                event => sessionState.contents.get(event?.sender));
            session.emit = sessionState.proxy;
            this.sessions.set(session, sessionState);
        }
        sessionState.contents.set(webContents, state);
        state.sessionState = sessionState;
        webContents.once("destroyed", () => this.dispose(state));
    }

    context(state, args) {
        return createContext(args, state.webContents, state.window ??
            this.electron.BrowserWindow.fromWebContents(state.webContents));
    }

    wrapEmitter(emit, resolveState) {
        const transport = this;
        return new Proxy(emit, {
            apply(target, thisArg, args) {
                if (args[0] !== "-ipc-message") return Reflect.apply(target, thisArg, args);
                const channelIndex = typeof args[2] === "boolean" ? 3 : 2;
                const channel = args[channelIndex];
                if ((channelIndex === 3 && args[2]) || typeof channel !== "string" ||
                    !channel.startsWith("RM_IPCTO_")) {
                    return Reflect.apply(target, thisArg, args);
                }
                const state = resolveState(args[1]);
                if (!state?.active) return Reflect.apply(target, thisArg, args);
                const payload = args[channelIndex + 1];
                const context = transport.context(state, [channel, ...payload]);
                return transport.interception.dispatch("outgoing", context, next => {
                    if (!state.active) return;
                    const forwarded = args.slice();
                    forwarded[channelIndex] = next[0];
                    const changed = payload.length !== next.length - 1 ||
                        payload.some((value, index) => value !== next[index + 1]);
                    forwarded[channelIndex + 1] = changed ? next.slice(1) : payload;
                    return Reflect.apply(target, thisArg, forwarded);
                });
            }
        });
    }

    dispose(state) {
        state.active = false;
        const { webContents, sessionState } = state;
        this.interception.disposeWebContents(state.id);
        if (webContents.send === state.sendProxy) webContents.send = state.send;
        if (webContents.emit === state.emitProxy) webContents.emit = state.emit;
        sessionState.contents.delete(webContents);
    }
}

function installTransport(electron, interception, onIncoming) {
    if (!electron.app[transportKey]) {
        Object.defineProperty(electron.app, transportKey, {
            value: new Transport(electron, interception, onIncoming)
        });
    }
    return electron.app[transportKey];
}

module.exports = { createContext, normalizeCommands, installTransport };
