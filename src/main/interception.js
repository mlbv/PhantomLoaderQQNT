// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const directions = ["incoming", "outgoing", "external"];

function isThenable(value) {
    return value != null && typeof value.then == "function";
}

class Interception {
    #handlers = new Map(directions.map(direction => [direction, []]));
    #streams = new Map(directions.map(direction => [direction, new Map()]));

    /**
     * Higher priorities run first; equal priorities retain registration order.
     * Filters run synchronously when dispatch starts. Disposing a registration
     * also skips its handlers in queued dispatches that have not entered it.
     */
    register(owner, direction, handler, options = {}) {
        const handlers = this.#getHandlers(direction);
        const { priority = 0, filter, webContentsId } = options;
        if (typeof handler != "function") throw new TypeError("An interception handler must be a function");
        if (!Number.isFinite(priority)) throw new TypeError("An interception priority must be a finite number");
        if (filter !== undefined && typeof filter != "function") {
            throw new TypeError("An interception filter must be a function");
        }
        if (webContentsId !== undefined && !Number.isInteger(webContentsId)) {
            throw new TypeError("A webContentsId must be an integer");
        }

        const registration = { owner, direction, handler, priority, filter, webContentsId, active: true };
        const index = handlers.findIndex(item => item.priority < priority);
        handlers.splice(index < 0 ? handlers.length : index, 0, registration);
        return () => this.#remove(registration);
    }

    disposeOwner(owner) {
        for (const handlers of this.#handlers.values()) {
            for (const registration of [...handlers]) {
                if (registration.owner === owner) this.#remove(registration);
            }
        }
    }

    /** Cancel pending dispatches after any already-entered handler settles. */
    disposeWebContents(id) {
        for (const [direction, handlers] of this.#handlers) {
            const stream = this.#streams.get(direction).get(id);
            if (stream) stream.lifetime.active = false;
            for (const registration of [...handlers]) {
                if (registration.webContentsId === id) this.#remove(registration);
            }
        }
    }

    /**
     * Only matched traffic joins its direction/webContents stream. Unmatched
     * traffic forwards synchronously, even while that stream has pending work.
     * Replacing args assigns context.args; transport accessors rebuild its views.
     */
    dispatch(direction, context, forward) {
        const handlers = this.#getHandlers(direction);
        if (!handlers.length) return forward(context.args);
        const registrations = [...handlers].filter(registration => {
            if (!registration.active) return false;
            if (registration.webContentsId !== undefined && registration.webContentsId !== context.webContents?.id) {
                return false;
            }
            if (!registration.filter) return true;
            const matches = registration.filter(context);
            if (isThenable(matches)) throw new TypeError("An interception filter must be synchronous");
            return Boolean(matches);
        });
        if (!registrations.length) return forward(context.args);

        const streams = this.#streams.get(direction);
        const id = context.webContents?.id;
        const previous = streams.get(id);
        const lifetime = previous?.lifetime ?? { active: true };
        let release;
        const stream = { done: new Promise(resolve => release = resolve), lifetime };
        streams.set(id, stream);
        const complete = () => {
            if (streams.get(id) === stream) streams.delete(id);
            release();
        };

        let result;
        try {
            result = previous ?
                previous.done.then(() => this.#run(registrations, context, forward, lifetime)) :
                this.#run(registrations, context, forward, lifetime);
            if (!isThenable(result)) {
                complete();
                return result;
            }
        }
        catch (error) {
            complete();
            throw error;
        }
        return Promise.resolve(result).then(value => {
            complete();
            return value;
        }, error => {
            complete();
            throw error;
        });
    }

    #getHandlers(direction) {
        const handlers = this.#handlers.get(direction);
        if (!handlers) throw new TypeError(`Unknown interception direction: ${direction}`);
        return handlers;
    }

    #remove(registration) {
        if (!registration.active) return;
        registration.active = false;
        const handlers = this.#handlers.get(registration.direction);
        handlers.splice(handlers.indexOf(registration), 1);
    }

    #run(registrations, context, forward, lifetime, index = 0) {
        for (; index < registrations.length; index++) {
            if (!lifetime.active) return;
            const registration = registrations[index];
            if (!registration.active) continue;
            const result = registration.handler(context);
            if (isThenable(result)) {
                return Promise.resolve(result).then(result => {
                    if (!lifetime.active || this.#apply(result, context)) return;
                    return this.#run(registrations, context, forward, lifetime, index + 1);
                });
            }
            if (!lifetime.active || this.#apply(result, context)) return;
        }
        if (lifetime.active) return forward(context.args);
    }

    #apply(result, context) {
        if (result === undefined) return false;
        if (result?.action === "cancel") return true;
        if (result?.action === "replace" && Array.isArray(result.args)) {
            context.args = result.args;
            return false;
        }
        throw new TypeError("An interception handler must return undefined, a replacement argument array, or cancellation");
    }
}

module.exports = { Interception, interception: new Interception() };
