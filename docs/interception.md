<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Main-process interception API

A plugin selecting `"api": "PhantomLoader"` receives that facade through `onLoad(PhantomLoader)`, after which `PhantomLoader.api.intercept` can transform messages or external URLs before they reach their forwarding calls. The API executes in the main process, while renderer IPC carries only the serialized operation data exposed by the transport.

~~~js
exports.onLoad = (PhantomLoader) => {
    const dispose = PhantomLoader.api.intercept.register(
        "my-plugin",
        "external",
        ({ args }) => {
            const url = new URL(args[0]);
            url.searchParams.delete("utm_source");
            return { action: "replace", args: [url.href, ...args.slice(1)] };
        }
    );
};
~~~

Call `dispose()` to remove one registration, or call `PhantomLoader.api.intercept.disposeOwner("my-plugin")` when the plugin needs to remove every registration associated with its owner.

## Registration

~~~js
register(owner, direction, handler, options = {})
~~~

The `owner` is the plugin slug, and `direction` selects the operation stream:

- `incoming`: main-to-renderer calls through `webContents.send`.
- `outgoing`: renderer-to-main QQ command dispatch.
- `external`: calls to `shell.openExternal`.

| Field | Meaning |
| --- | --- |
| priority | Numeric execution priority, highest first. The default is zero, and equal priorities retain registration order. |
| filter | Synchronous predicate receiving the interception context; a true result includes the registration in the operation. |
| webContentsId | Web contents scope for the registration. |

Filters therefore select the traffic a plugin transforms, and the loader evaluates them when an operation enters the registry.

## Context

Handlers receive the following context:

| Field | Meaning |
| --- | --- |
| args | Original arguments to the intercepted operation, including its channel where applicable. |
| channel | IPC channel where applicable. |
| window | Associated browser window where applicable. |
| webContents | Associated web contents where applicable. |
| commands | Normalized QQ command views with name and payload fields. |

Incoming and outgoing IPC uses `args` in the form `[channel, ...qqArguments]`, whereas external URLs use `[url, ...options]`. Native Electron event objects stay in the transport adapter and remain available during forwarding, while QQ envelopes are interpreted there and command views expose their names and payloads without removing the raw arguments needed for response handling.

## Results and ordering

A handler may return a direct result or a Promise:

- `undefined` continues with the current arguments, including in-place edits.
- `{ action: "replace", args }` replaces the complete argument list.
- `{ action: "cancel" }` cancels the complete operation and its command batch.

Handlers run in priority order, and the transport reconstructs command views after argument replacement. The loader performs one final forwarding call for each completed operation, cancellation stops forwarding and handler errors propagate to the caller.

Synchronous transformations complete synchronously. Promise-returning handlers queue later matched work for the same direction and web-contents stream, allowing unrelated traffic to continue through its own stream; synchronous IPC remains on the synchronous transport path.

A handler cannot dispatch another matched operation into its own active direction and web-contents stream. Such a dispatch throws because awaiting its completion would prevent the current handler from releasing the stream. Unmatched operations can still forward immediately.

## Lifetime

The registry skips a registration removed before an operation starts, while JavaScript already executing inside a handler continues to completion. Destroying web contents removes its bound registrations and cancels pending forwarding for that web contents, although an active handler may finish before the destroyed target is released.

Native addons retain Node's ordinary addon loading path, and the interception registry neither loads nor manages those modules.
