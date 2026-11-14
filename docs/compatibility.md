<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Plugin API selection

PhantomLoaderQQNT 1.0.0 provides the LiteLoader 5.2.1 API together with the extended PhantomLoader API. Each plugin selects its facade through the optional `api` field in manifest v4; when the field is absent, the loader selects LiteLoader so existing manifests continue to use their established surface.

~~~json
{
    "api": "PhantomLoader"
}
~~~

After evaluating a plugin's entry module, the loader calls its synchronous `onLoad(api)` initializer before registering lifecycle hooks. Main, preload and renderer entries may each export their own initializer, and the supplied object remains available through ordinary variables and closures.

~~~js
// Main or preload entry
let loader;

exports.onLoad = (api) => {
    loader = api;
};

// Renderer entry
export function onLoad(api) {
    loader = api;
}
~~~

LiteLoader plugins receive paths, the plugin registry, LiteLoader and QQ package metadata, runtime versions, configuration and plugin-management operations, together with `openExternal` and `openPath`. The LiteLoader global exposes the same legacy surface. A plugin selecting PhantomLoader receives those shared state and legacy operations alongside PhantomLoader package and version metadata, and its main-process facade additionally exposes `api.intercept` and `api.native`.

| Property | Value |
| --- | --- |
| PhantomLoader.versions.phantomloader | "1.0.0" |
| PhantomLoader.package.phantomloader.version | "1.0.0" |
| LiteLoader.versions.liteloader | "5.2.1" |
| LiteLoader.package.liteloader.version | "5.2.1" |

Here, PhantomLoader names the object passed to an opted-in plugin's initializer. Its main-process native binding is `phantomloader_native`, with `liteloader_native` retained as an alias, while preload and renderer initializers receive serializable metadata together with configuration, plugin-management and shell-bridge operations.

`PHANTOMLOADERQQNT_PROFILE` takes precedence over `LITELOADERQQNT_PROFILE`; both variables select a profile containing plugin and data directories. Core configuration uses `data/LiteLoader/config.json` for either reserved loader slug, and plugin registration, dependency resolution, existing lifecycle exports and manifest v4 retain their established contracts.

The `local://` resource scheme and legacy settings CSS classes remain available, while settings navigation and content carry the `phantomloader` class. The preload bridge routes the two facades through their respective `PhantomLoader.PhantomLoader.PhantomLoader` and `LiteLoader.LiteLoader.LiteLoader` channels.
