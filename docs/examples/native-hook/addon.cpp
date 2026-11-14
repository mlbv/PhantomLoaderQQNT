// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

#include <windows.h>
#define NAPI_EXTERN
#include <node_api.h>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <utility>

struct alignas(16) Frame {
    uint64_t registers[7];
    uint64_t padding;
    uint8_t xmm[6][16];
    uint8_t* stack;
    uint64_t stack_bytes;
    uint64_t token;
    uint64_t user_data;
};
static_assert(sizeof(Frame) == 192);
static_assert(alignof(Frame) == 16);
static_assert(offsetof(Frame, stack) == 160);

using InvokeOriginal = uint32_t (*)(uint64_t, Frame*);

struct State {
    std::atomic<InvokeOriginal> invoke{nullptr};
    std::atomic<uint64_t> calls{0};
};

// Pinned with the addon: callbacks never depend on a live napi_env or JS object.
static State state;

static void on_call(Frame* frame) {
    auto& context = *reinterpret_cast<State*>(frame->user_data);
    context.calls.fetch_add(1, std::memory_order_relaxed);
    context.invoke.load(std::memory_order_acquire)(frame->token, frame);
}

struct Api {
    decltype(&napi_get_cb_info) get_cb_info;
    decltype(&napi_get_value_bigint_uint64) get_word;
    decltype(&napi_create_bigint_uint64) create_word;
    decltype(&napi_create_uint32) create_uint32;
    decltype(&napi_create_function) create_function;
    decltype(&napi_set_named_property) set_property;
    decltype(&napi_throw_error) throw_error;
    decltype(&napi_get_version) get_version;
};

static const Api& api() {
    static const Api functions = [] {
        const auto host = GetModuleHandleW(L"QQNT.dll");
        return Api{
            reinterpret_cast<decltype(Api::get_cb_info)>(GetProcAddress(host, "napi_get_cb_info")),
            reinterpret_cast<decltype(Api::get_word)>(GetProcAddress(host, "napi_get_value_bigint_uint64")),
            reinterpret_cast<decltype(Api::create_word)>(GetProcAddress(host, "napi_create_bigint_uint64")),
            reinterpret_cast<decltype(Api::create_uint32)>(GetProcAddress(host, "napi_create_uint32")),
            reinterpret_cast<decltype(Api::create_function)>(GetProcAddress(host, "napi_create_function")),
            reinterpret_cast<decltype(Api::set_property)>(GetProcAddress(host, "napi_set_named_property")),
            reinterpret_cast<decltype(Api::throw_error)>(GetProcAddress(host, "napi_throw_error")),
            reinterpret_cast<decltype(Api::get_version)>(GetProcAddress(host, "napi_get_version")),
        };
    }();
    return functions;
}

#define NAPI_TRY(expression) \
    do { \
        if ((expression) != napi_ok) { \
            api().throw_error(env, nullptr, "Native hook example: Node-API call failed"); \
            return nullptr; \
        } \
    } while (false)

static napi_value word(napi_env env, uint64_t value) {
    napi_value result;
    NAPI_TRY(api().create_word(env, value, &result));
    return result;
}

static napi_value configure(napi_env env, napi_callback_info info) {
    size_t count = 1;
    napi_value argument;
    NAPI_TRY(api().get_cb_info(env, info, &count, &argument, nullptr, nullptr));
    if (count != 1) {
        api().throw_error(env, nullptr, "Expected native.invokeOriginal");
        return nullptr;
    }
    uint64_t address;
    bool lossless;
    NAPI_TRY(api().get_word(env, argument, &address, &lossless));
    if (!lossless || address == 0) {
        api().throw_error(env, nullptr, "Expected a nonzero unsigned native address");
        return nullptr;
    }
    state.invoke.store(reinterpret_cast<InvokeOriginal>(address), std::memory_order_release);
    return word(env, reinterpret_cast<uint64_t>(&state));
}

static napi_value calls(napi_env env, napi_callback_info) {
    return word(env, state.calls.load(std::memory_order_relaxed));
}

static napi_value probe(napi_env env, napi_callback_info) {
    uint32_t version;
    NAPI_TRY(api().get_version(env, &version));
    napi_value result;
    NAPI_TRY(api().create_uint32(env, version, &result));
    return result;
}

NAPI_MODULE_INIT() {
    HMODULE pinned;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            reinterpret_cast<LPCWSTR>(&on_call), &pinned)) {
        api().throw_error(env, nullptr, "Cannot pin the native callback module");
        return nullptr;
    }
    const auto callback = word(env, reinterpret_cast<uint64_t>(&on_call));
    if (!callback) return nullptr;
    NAPI_TRY(api().set_property(env, exports, "callback", callback));
    for (const auto& method : {
            std::pair<const char*, napi_callback>{"configure", configure},
            {"calls", calls},
            {"probe", probe}}) {
        napi_value function;
        NAPI_TRY(api().create_function(env, method.first, NAPI_AUTO_LENGTH,
            method.second, nullptr, &function));
        NAPI_TRY(api().set_property(env, exports, method.first, function));
    }
    return exports;
}
