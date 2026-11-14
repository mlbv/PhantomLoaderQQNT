// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

const helper = require("./build/Release/qqnt_hook.node");

let native;
let hook;
let stopped = false;

exports.onLoad = (PhantomLoader) => {
    native = PhantomLoader.api.native;
    if (native.apiVersion !== 1) throw new Error("Expected native API version 1");

    const target = native.getExport("QQNT.dll", "napi_get_version");
    const userData = helper.configure(native.invokeOriginal);
    hook = native.attach(target, helper.callback, 0, userData);

    try {
        const before = helper.calls();
        const version = helper.probe();
        const observed = helper.calls() - before;
        if (observed === 0n) throw new Error("The native callback was not reached");
        console.log("[native_hook_example]", { version, observed });
    } finally {
        console.log("[native_hook_example] detached; drained:", exports.stop());
    }
};

exports.stop = () => {
    if (!hook) return true;
    if (!stopped) {
        native.detach(hook.id);
        stopped = true;
    }
    return native.isDrained(hook.id);
};
