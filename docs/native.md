<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Native hooks for plugins

A native hook lets a plugin observe or replace a function inside QQNT's main process, combining JavaScript installation with a callback on the intercepted native thread. That callback can inspect and modify arguments, invoke the original function, and adjust its result before execution returns to the original native caller.

This guide develops a complete plugin before specifying its API and callback ABI, assuming Windows 11 x64 and native API version 1 throughout. For JavaScript messages and external links, use the [interception API](interception.md); for the loader's implementation and process boundaries, read [Native runtime architecture](../native/README.md).

## A complete example

The [example plugin](examples/native-hook) hooks `napi_get_version`, an export of `QQNT.dll`, and counts intercepted calls while forwarding their arguments and results unchanged to the original caller. A native helper exercises that export with a valid Node environment, making interception observable without depending on a particular QQ action or undocumented function signature.

After demonstrating interception, the plugin detaches its hook, while its process-lifetime callback storage remains valid for any execution that has already entered the callback. The four source files separate the plugin declaration, JavaScript installation, native callback and build configuration so that each part can be adapted independently.

| File | Purpose |
| --- | --- |
| [manifest.json](examples/native-hook/manifest.json) | Selects the PhantomLoader API and main-process entry. |
| [main.js](examples/native-hook/main.js) | Configures the helper, attaches, exercises the target and detaches. |
| [addon.cpp](examples/native-hook/addon.cpp) | Supplies the callback and native state, and invokes the target for the demonstration. |
| [CMakeLists.txt](examples/native-hook/CMakeLists.txt) | Builds the helper as a Windows x64 Node-API addon. |

### Build and install

Building the example requires Visual Studio C++ build tools, CMake and a Node-API header directory containing `node_api.h`, such as the headers distributed with Node.js. Because the helper uses Node-API version 8 and resolves host functions from `QQNT.dll`, it requires neither `node.lib` nor linkage against the V8 C++ ABI.

Copy the example directory into your plugin workspace, then run the following commands there, substituting the directory that contains your local Node-API header files:

```powershell
cmake -S . -B build -A x64 "-DNODE_INCLUDE_DIR=C:/path/to/node/include/node"
cmake --build build --config Release
```

Install the directory containing `manifest.json`, `main.js` and `build/Release/qqnt_hook.node` under your profile's `plugins` directory, enable the plugin, and restart QQ through the PhantomLoader shortcut. The main-process log should report a Node-API version, a positive `observed` count and the detach/drain result, although concurrent callers can increase the count beyond one.

Because the helper resolves a QQNT-hosted export, the example runs inside QQNT rather than an ordinary Node process, and demonstrates forwarding for that execution. The plugin author must still establish target patchability, including for this export, since the loader's compatibility profile does not certify arbitrary plugin hook sites.

### What runs where

The manifest selects `"api": "PhantomLoader"` and names `main.js` as its main entry, which the loader evaluates before synchronously calling `onLoad(PhantomLoader)` and starting QQ's application launcher. Native operations are available only in that main-process entry; preload and renderer entries receive their own facades without access to the native binding.

The installation sequence loads the callback helper, configures its original-invocation entry, resolves the target and attaches the hook before retaining its ID for later cleanup:

```js
const helper = require("./build/Release/qqnt_hook.node");

exports.onLoad = (PhantomLoader) => {
    const native = PhantomLoader.api.native;
    const userData = helper.configure(native.invokeOriginal);
    const target = native.getExport("QQNT.dll", "napi_get_version");
    const hook = native.attach(target, helper.callback, 0, userData);
    // Retain hook.id until this attachment is no longer needed.
};
```

The `attach(target, callback, stackArgumentBytes, userData)` operation returns `{ id }`, using unsigned BigInts for addresses, user data and the ID, but a Number for stack extent. Configuring the helper first ensures that its original-invocation entry is published before any native thread can reach the callback through the newly attached hook.

For this target, whose signature is `napi_status napi_get_version(napi_env, uint32_t*)`, RCX and RDX carry the two pointers and no arguments occupy later stack positions. Accordingly, the example passes zero for `stackArgumentBytes`; when the helper exercises that export, the resulting native call reaches the callback shown below.

```cpp
static void on_call(Frame* frame) {
    auto& context = *reinterpret_cast<State*>(frame->user_data);
    context.calls.fetch_add(1, std::memory_order_relaxed);
    context.invoke.load(std::memory_order_acquire)(frame->token, frame);
}
```

The callback increments its native counter before invoking the original with the frame's capability, which preserves the captured arguments and receives the original's output registers. When the callback returns, the dispatcher restores those outputs for the native caller, allowing this observer to preserve the target's ordinary behavior and result.

The callback performs no Node-API or JavaScript work, whereas the helper's `probe()` method runs on the JavaScript thread and supplies its actual `napi_env`. A fabricated environment passed through `native.call` would violate the target's contract, even if the pointer value could be represented correctly as a BigInt.

Read [the example source](examples/native-hook/addon.cpp) for complete declarations and error handling; its helper pins the DLL and retains callback state for the entire process lifetime. That lifetime ensures that neither environment teardown nor an in-flight execution invalidates the callback's code or data, simplifying the example's cleanup without requiring a separate reclamation mechanism.

### Adapting the example to a QQ function

Before adapting the example, establish the exact module, supported QQ build and native signature; exported functions can then be resolved directly through `getExport`. For an unexported function, the native helper must locate its loaded module, validate its identity and relevant bytes, and derive an address from a reviewed RVA or another reliable locator. An RVA is relative to the loaded module rather than an absolute address or necessarily a file offset, and this API supplies no general module-base lookup, scanner or memory reader.

Install the hook after its module loads but before the work you need to observe, accounting for native initialization that may already precede plugin startup. Although `QQNT.dll` is available during main-process `onLoad`, later modules may require an application lifecycle point or native load observation implemented by your helper.

Because the callback can execute concurrently or recursively on any application thread calling the target, its state and forwarding logic must support those execution patterns. To modify an argument, edit the relevant frame slot before invoking the original; to modify the result, edit its return slot after forwarding completes. To replace execution entirely, supply the required return value and return without forwarding, preserving any additional effects required by the function's established native contract.

For a scalar integer return, for example, assigning `frame->registers[0] = 1;` after forwarding changes RAX, provided that value is meaningful for the target's return contract. Retain the hook ID for cleanup, and expose a stop operation through your own control path when the attachment is no longer required by the plugin.

The example's exported `stop()` is an ordinary function used by its demonstration, rather than an unload lifecycle hook recognized automatically by the plugin loader. For a long-lived attachment, remove the immediate `finally` cleanup and invoke that operation from the control path responsible for ending the hook's useful lifetime.

## Choosing a patchable target

The plugin author must establish target patchability: after resolving supported import and jump aliases, the loader overwrites complete instructions covering at least five bytes. While attached, execution arriving from outside that prefix may enter only at its first byte, excluding interior transfers through branches, alternate entries, returns or restored contexts. Overlapping instruction streams are unsupported, and the target's original code must remain stable throughout attachment so that relocation and later restoration remain valid.

Although the loader's branch scan detects some violations, successful attachment does not establish this single-entry property or relieve the author of validating the target. An incorrect address, signature, stack extent or entry assumption causes undefined behavior and can corrupt native execution, so target validation must be repeated when QQNT changes.

For `NtMapViewOfSection` and `NtMapViewOfSectionEx`, attachment composes with the resident image observer. Forwarding through `invokeOriginal` completes the native mapping and mandatory profile processing before the mapped image returns to the callback; detaching the plugin leaves the observer installed.

Only one active attachment is supported for a resolved target and its aliases, and the target and callback must resolve to distinct native functions. Preparation and activation also enforce the following limits, whose Win32 error codes distinguish a stopped execution from invalid input or unsupported relocation and unwind behavior.

| Result | Meaning |
| --- | --- |
| `ERROR_RETRY` / `WIN32_1237` | A suspended native instruction pointer lies inside the entry span. Attachment did not commit. |
| `ERROR_INVALID_PARAMETER` / `WIN32_87` | Invalid input or an unsupported/invalid relocation, including some detected branches into the span. |
| `ERROR_NOT_SUPPORTED` / `WIN32_50` | A copied CALL would return into the overwritten span, or the source uses unsupported frame/unwind behavior. |

A copied CALL may return immediately beyond the overwritten span, but a return into its interior is unsupported because activation cannot safely admit that pre-existing continuation. When a prefix has unwind operations, copied body instructions must preserve its source frame, excluding returns and writes to RSP or the active frame register. Chained unwind metadata and copied instructions within exception- or termination-handler-protected bodies are also unsupported, so a valid function entry may still be rejected during preparation.

## Callback ABI and original calls

The callback uses the Windows x64 signature `void callback(Frame*)`, executes synchronously on the intercepted thread, and supplies native results by editing the frame. It must not call into JavaScript, since the intercepted thread and call site do not provide the environment or scheduling contract required for JavaScript execution.

```cpp
struct alignas(16) Frame {
    uint64_t registers[7]; // RAX, RCX, RDX, R8, R9, R10, R11
    uint64_t padding;
    uint8_t xmm[6][16];
    uint8_t* stack;
    uint64_t stack_bytes;
    uint64_t token;
    uint64_t user_data;
};
using InvokeOriginal = uint32_t (*)(uint64_t token, Frame* frame);
```

The 192-byte, 16-byte-aligned `Frame` carries the supplied `user_data` and an opaque original-call `token`, whose value is a capability rather than a generated code address. R10 is reserved for loader routing and must not be interpreted as a hook ID or the original incoming register value; other captured volatile registers retain their native inputs.

| Target value | Frame location |
| --- | --- |
| Integer/pointer arguments in the first four positions | `registers[1]` through `registers[4]` (RCX, RDX, R8, R9). |
| Floating-point arguments in the first four positions | Corresponding XMM0–3 slots, following the target's Windows x64 signature. |
| Arguments after the first four positions | The bytes addressed by `stack`. |
| Scalar integer/pointer return | `registers[0]` (RAX). |
| Floating-point return | `xmm[0]` (XMM0). |

The `stackArgumentBytes` Number specifies bytes after the first four argument positions, including argument padding but excluding the 32-byte shadow space required by Windows x64. Its value must be an unsigned 32-bit multiple of eight; a target with six ordinary 64-bit integer arguments therefore requires 16 bytes of stack arguments. Hidden arguments, such as structure-return pointers, affect placement, so aggregate and variadic signatures must be interpreted according to their actual native ABI before configuring the hook.

The immutable `native.invokeOriginal` BigInt identifies a resident function with the `InvokeOriginal` signature, whose status is separate from the target's return value in the frame. Status `0` indicates that forwarding completed and captured output registers, whereas `1` rejects an invalid or expired token, incorrect callback thread, or mismatched stack extent. The original's Win32 last-error value survives the loader's return bookkeeping, allowing forwarding callbacks to preserve that part of the target's behavior as well.

During a callback, `frame->stack` addresses the caller's fifth argument, and forwarding copies the configured bytes into a private native argument area for the original. To modify stack arguments without changing the caller's storage, copy them into a callback-owned buffer and redirect the frame's pointer before invoking the original. Writes into the original's private argument area are not copied back, although writes through pointer arguments retain their normal effects on the referenced native memory.

A callback token permits repeated original invocations on its native thread until the callback returns or unwinds, after which the capability expires and cannot admit another call. Calling the target's hooked entry directly would reenter the hook, whereas `invokeOriginal` forwards through the retained original path; application calls made by that original remain observable. Calls made internally by the loader bypass plugin callbacks, including transitive calls from its allocator, synchronization and coordinator, so internal work does not recursively invoke application callbacks.

For original calls that must occur outside the callback, acquire an independent capability and retain it until native code no longer needs to start additional calls:

```js
const token = native.acquireOriginal(hook.id);
// Give token and native.invokeOriginal to native code.
// Keep the capability until that code no longer needs to start original calls.
native.releaseOriginal(token);
```

The native caller supplies an aligned `Frame`, initializes arguments and `stack_bytes`, provides valid stack storage where needed, and passes the capability as `InvokeOriginal`'s first argument. Independent capabilities can be used across threads and remain usable after detach until released, at which point new admissions stop while already-admitted calls retain ownership.

## Detach and shutdown

The `native.detach(id)` operation restores the target and prevents new callback admissions, returning before callbacks or originals already in progress have necessarily completed their execution. Existing callback tokens and independent capabilities preserve their documented uses until they expire or are released, so detachment alone does not permit callback resources to be freed.

After stopping native producers of independent original calls, release their capabilities and check `native.isDrained(id)`, which becomes true when detachment and all retained uses have completed. Callback code and user data must remain alive until that point; if the result is false, retain them because the API supplies no JavaScript completion event. The example satisfies this requirement with process-lifetime callback storage, even when another thread causes its immediate drain check to return false after the demonstration.

Attachments and independent capabilities belong to their creating Node environment, which can contain several plugins; each plugin should therefore close the IDs it owns individually. By contrast, `native.dispose()` closes all hooks and revokes independent capabilities throughout that environment, potentially affecting other plugins, without waiting for admitted native executions to finish.

Environment teardown performs the same close/revoke operation and waits for application executions outside its enclosing native stack, while enclosing calls and loader-internal executions retain their own lifetimes. Any callback or callee continuing afterward must keep its code and data alive independently of the destroyed environment, avoid accessing that environment, and finish without JavaScript scheduled on it. If attachment succeeds but publishing its JavaScript result fails, the loader closes and fully drains the unpublished attachment before returning the error to JavaScript.

Normal returns and Windows exception unwinding release execution ownership, whereas transfers that discard dispatcher frames without Windows unwinding, including `longjmp` and thread termination, violate the contract. Continuations copied outside the payload, such as debugger contexts or fiber stacks, acquire no ownership and must not be replayed into generated code after drain.

The launcher must remain running throughout QQ's lifetime, and target modules must remain loaded while ordinary native callers can use them, including after detachment completes. Hook drain establishes when callback resources can be released rather than when a target module is quiescent; previously hooked addresses must also avoid intrinsic INT3 entries for the remaining process lifetime.

## Other binding operations

The getter `PhantomLoader.api.native` lazily obtains `process._linkedBinding("phantomloader_native")`, while the compatibility binding `liteloader_native` returns the same export object within a given Node environment. Addresses, IDs, capabilities and native integer words cross the JavaScript boundary as unsigned 64-bit BigInts, preserving their values independently of JavaScript Number precision limits.

| Member | Contract |
| --- | --- |
| `apiVersion` | Native API contract version, currently `1`. |
| `platformVersion` | Platform release version. |
| `targetProfileSha256` | SHA-256 of the exact compatibility profile compiled into the payload. |
| `pointerSize` | Native pointer size in bytes, currently `8`. |
| `payloadBase` | Mapped payload address as a BigInt. |
| `getExport(moduleName, exportName)` | Resolve an export in an already loaded module and return its address. An empty module name selects the main executable. |
| `call(address, args)` | Call a Windows x64 function with zero to four unsigned 64-bit integer/pointer arguments, returning an unsigned 64-bit integer. It does not marshal floating-point arguments, strings or JavaScript buffers. |

For example, resolving an already loaded Windows export and invoking its zero-argument integer signature requires only the address and an empty argument array:

```js
const address = native.getExport("kernel32.dll", "GetTickCount64");
const ticks = native.call(address, []);
```

All addresses and referenced memory must remain valid for the declared signature, while hook operations report Win32 failures as JavaScript errors carrying `WIN32_<number>` codes. The compatibility profile is distributed as `loader/distribution/profile.json`, and its exact bytes must agree with the profile compiled into both the launcher and native payload.

## DLVM capture

Core startup arms the process-wide DLVM guard before plugins load, so plugins normally leave arming to the core unless capture has explicitly been disabled. Setting `enable_dlvm_capture: false` disables arming on the next launch, while an already armed guard retains its process lifetime and survives subsequent Node environment cleanup.

The `native.armDlVmCapture(outputDirectory)` operation performs one-shot arming for an existing absolute directory whose path contains at most 160 UTF-16 code units in total. The caller verifies the on-disk wrapper fingerprint before arming, while the payload independently checks the loaded image and hook prologues against the supported compatibility profile. Successful arming returns the current state, setup failures throw, and `native.dlVmCaptureStatus()` reads that state without repeating initialization, using the values listed in the following table.

| State | Meaning |
| --- | --- |
| `0` | Uninitialized. |
| `1` | Waiting for the wrapper image. |
| `2` | Wrapper verified and capture hooks armed. |
| `3` | Capture setup failed. |

The guard records status in `guard-status.json` and saves matching programs as raw bytes with JSON metadata before terminating QQ and allowing post-exit reporting to begin. Exit codes distinguish capture success (`0xD1500001`), dump failure (`0xD1500002`) and arming failure (`78`), which the launcher handles by opening the independent report UI. The guard ends with the process rather than environment cleanup; [the architecture guide](../native/README.md#dlvm-capture-and-reporting) explains how capture hooks and post-exit reporting fit together.
