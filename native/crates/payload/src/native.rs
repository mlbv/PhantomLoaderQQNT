// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use core::{
    ffi::{CStr, c_void},
    fmt::Write,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};
use platform::{
    incident::MAX_CAPTURE_DIRECTORY_UNITS,
    profile::{NATIVE_API_VERSION, PLATFORM_VERSION, PROFILE_SHA256},
};
use windows_sys::Win32::Foundation::{HMODULE, MAX_PATH};

use crate::{
    capture,
    formatting::Text,
    managed,
    napi::{self, AUTO_LENGTH, CallbackInfo, Env, Module, OK, Property, Value},
    runtime,
};

const WIN64_INTEGER_ARGUMENTS: usize = 4;
const EXPORT_NAME_BUFFER_BYTES: usize = 256;

static REGISTERED: AtomicBool = AtomicBool::new(false);
static mut MODULE: Module = Module::linked(c"phantomloader_native", initialize);
static mut LEGACY_MODULE: Module = Module::linked(c"liteloader_native", initialize_alias);

macro_rules! checked {
    ($expression:expr) => {
        if $expression != OK {
            return ptr::null_mut();
        }
    };
}

unsafe fn type_error(env: Env, message: &CStr) -> Value {
    unsafe {
        (napi::api().throw_type_error)(env, ptr::null(), message.as_ptr());
    }
    ptr::null_mut()
}

unsafe fn range_error(env: Env, message: &CStr) -> Value {
    unsafe {
        (napi::api().throw_range_error)(env, ptr::null(), message.as_ptr());
    }
    ptr::null_mut()
}

unsafe fn arguments<const N: usize>(env: Env, info: CallbackInfo, args: &mut [Value; N]) -> bool {
    unsafe {
        let required = N - 1;
        let mut count = N;
        if (napi::api().get_cb_info)(
            env,
            info,
            &mut count,
            args.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        ) != OK
        {
            return false;
        }
        if count != required {
            type_error(env, c"Incorrect number of arguments");
            return false;
        }
        true
    }
}

unsafe fn get_word(env: Env, value: Value) -> Option<u64> {
    unsafe {
        let mut word = 0;
        let mut lossless = false;
        if (napi::api().get_value_bigint_uint64)(env, value, &mut word, &mut lossless) != OK
            || !lossless
        {
            type_error(env, c"Expected an unsigned 64-bit BigInt");
            return None;
        }
        Some(word)
    }
}

unsafe fn word(env: Env, word: u64) -> Value {
    unsafe {
        let mut result = ptr::null_mut();
        checked!((napi::api().create_bigint_uint64)(env, word, &mut result));
        result
    }
}

unsafe fn unsigned(env: Env, value: u32) -> Value {
    unsafe {
        let mut result = ptr::null_mut();
        checked!((napi::api().create_uint32)(env, value, &mut result));
        result
    }
}

unsafe fn undefined(env: Env) -> Value {
    unsafe {
        let mut result = ptr::null_mut();
        checked!((napi::api().get_undefined)(env, &mut result));
        result
    }
}

unsafe fn hook_error(env: Env, error: u32) -> Value {
    unsafe {
        let mut code = Text::<{ "WIN32_".len() + u32::MAX.ilog10() as usize + 1 + 1 }>::new();
        let _ = write!(code, "WIN32_{error}\0");
        (napi::api().throw_error)(
            env,
            code.as_bytes().as_ptr().cast(),
            c"Native hook operation failed".as_ptr(),
        );
        ptr::null_mut()
    }
}

unsafe fn fatal(message: &CStr) -> ! {
    unsafe {
        (napi::api().fatal_error)(
            c"phantomloader_native".as_ptr(),
            AUTO_LENGTH,
            message.as_ptr(),
            AUTO_LENGTH,
        )
    }
}

unsafe extern "C" fn get_export(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 3];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let api = napi::api();
        let mut module_length = 0;
        let mut symbol_length = 0;
        if (api.get_value_string_utf16)(env, args[0], ptr::null_mut(), 0, &mut module_length) != OK
            || (api.get_value_string_utf8)(env, args[1], ptr::null_mut(), 0, &mut symbol_length)
                != OK
        {
            return type_error(env, c"Expected module name and export name strings");
        }
        let mut module = [0u16; MAX_PATH as usize];
        let mut symbol = [0u8; EXPORT_NAME_BUFFER_BYTES];
        if module_length >= module.len() || symbol_length >= symbol.len() {
            return range_error(env, c"Module or export name is too long");
        }
        checked!((api.get_value_string_utf16)(
            env,
            args[0],
            module.as_mut_ptr(),
            module.len(),
            &mut module_length
        ));
        checked!((api.get_value_string_utf8)(
            env,
            args[1],
            symbol.as_mut_ptr().cast(),
            symbol.len(),
            &mut symbol_length
        ));
        let host = runtime::api();
        let handle = (host.get_module_handle_w)(if module_length == 0 {
            ptr::null()
        } else {
            module.as_ptr()
        });
        if handle.is_null() {
            (api.throw_error)(env, ptr::null(), c"Module is not loaded".as_ptr());
            return ptr::null_mut();
        }
        let Some(address) = (host.get_proc_address)(handle, symbol.as_ptr().cast()) else {
            (api.throw_error)(env, ptr::null(), c"Export does not exist".as_ptr());
            return ptr::null_mut();
        };
        word(env, address as usize as u64)
    }
}

unsafe extern "C" fn call(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 3];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let Some(address) = get_word(env, args[0]) else {
            return ptr::null_mut();
        };
        let api = napi::api();
        let mut is_array = false;
        if (api.is_array)(env, args[1], &mut is_array) != OK || !is_array {
            return type_error(env, c"Native arguments must be an array");
        }
        let mut count = 0;
        checked!((api.get_array_length)(env, args[1], &mut count));
        let mut words = [0u64; WIN64_INTEGER_ARGUMENTS];
        if count as usize > words.len() {
            return range_error(env, c"Win64 integer calls support at most four arguments");
        }
        for index in 0..count {
            let mut item = ptr::null_mut();
            checked!((api.get_element)(env, args[1], index, &mut item));
            let Some(value) = get_word(env, item) else {
                return ptr::null_mut();
            };
            words[index as usize] = value;
        }
        let function: unsafe extern "system" fn(u64, u64, u64, u64) -> u64 =
            core::mem::transmute(address as usize);
        word(env, function(words[0], words[1], words[2], words[3]))
    }
}

#[cfg(feature = "fixture-api")]
static FAIL_ATTACH_RESULT: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "fixture-api")]
#[unsafe(no_mangle)]
pub extern "C" fn FixtureFailNextAttachResult() {
    FAIL_ATTACH_RESULT.store(true, Ordering::Release);
}

unsafe extern "C" fn attach(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 5];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let Some(target) = get_word(env, args[0]) else {
            return ptr::null_mut();
        };
        let Some(callback) = get_word(env, args[1]) else {
            return ptr::null_mut();
        };
        let Some(user_data) = get_word(env, args[3]) else {
            return ptr::null_mut();
        };
        if target == 0 || callback == 0 || target == callback {
            return type_error(
                env,
                c"Expected distinct nonzero native target and callback addresses",
            );
        }
        let api = napi::api();
        let mut stack_bytes = 0.0;
        if (api.get_value_double)(env, args[2], &mut stack_bytes) != OK {
            return type_error(env, c"Expected stack argument byte count as a number");
        }
        if !(stack_bytes >= 0.0 && stack_bytes <= u32::MAX as f64)
            || stack_bytes as u32 as f64 != stack_bytes
            || stack_bytes as u32 % 8 != 0
        {
            return range_error(
                env,
                c"Stack argument byte count must be an unsigned 32-bit multiple of eight",
            );
        }
        let mut result = ptr::null_mut();
        checked!((api.create_object)(env, &mut result));
        let attachment = match managed::attach(
            env,
            target as usize,
            callback as usize,
            stack_bytes as u64,
            user_data,
        ) {
            Ok(id) => id,
            Err(error) => return hook_error(env, error),
        };
        #[cfg(feature = "fixture-api")]
        let inject_failure = FAIL_ATTACH_RESULT.swap(false, Ordering::AcqRel);
        #[cfg(not(feature = "fixture-api"))]
        let inject_failure = false;
        let id = if inject_failure {
            type_error(env, c"Injected Node-API result allocation failure")
        } else {
            word(env, attachment)
        };
        if id.is_null() || (api.set_named_property)(env, result, c"id".as_ptr(), id) != OK {
            if managed::rollback_attach(env, attachment).is_err() {
                fatal(c"Failed to roll back hook after Node-API allocation failure");
            }
            return ptr::null_mut();
        }
        result
    }
}

unsafe extern "C" fn detach(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 2];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let Some(id) = get_word(env, args[0]) else {
            return ptr::null_mut();
        };
        if let Err(error) = managed::detach(env, id) {
            return hook_error(env, error);
        }
        undefined(env)
    }
}

unsafe extern "C" fn dispose(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 1];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        if let Err(error) = managed::dispose(env) {
            return hook_error(env, error);
        }
        undefined(env)
    }
}

unsafe extern "C" fn acquire_original(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 2];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let Some(id) = get_word(env, args[0]) else {
            return ptr::null_mut();
        };
        let token = match managed::acquire_original(env, id) {
            Ok(token) => token,
            Err(error) => return hook_error(env, error),
        };
        let result = word(env, token);
        if result.is_null() && managed::release_original(env, token).is_err() {
            fatal(c"Failed to release original lease after Node-API allocation failure");
        }
        result
    }
}

unsafe extern "C" fn release_original(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 2];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let Some(token) = get_word(env, args[0]) else {
            return ptr::null_mut();
        };
        if let Err(error) = managed::release_original(env, token) {
            return hook_error(env, error);
        }
        undefined(env)
    }
}

unsafe extern "C" fn is_drained(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 2];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let Some(id) = get_word(env, args[0]) else {
            return ptr::null_mut();
        };
        let drained = match managed::is_drained(env, id) {
            Ok(drained) => drained,
            Err(error) => return hook_error(env, error),
        };
        let mut result = ptr::null_mut();
        checked!((napi::api().get_boolean)(env, drained, &mut result));
        result
    }
}

unsafe extern "C" fn arm_capture(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 2];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        let api = napi::api();
        let mut length = 0;
        if (api.get_value_string_utf16)(env, args[0], ptr::null_mut(), 0, &mut length) != OK {
            return type_error(env, c"Expected an absolute capture-directory string");
        }
        if length == 0 || length > MAX_CAPTURE_DIRECTORY_UNITS {
            return range_error(env, c"Capture directory must contain 1 to 160 characters");
        }
        let mut directory = [0u16; MAX_PATH as usize];
        checked!((api.get_value_string_utf16)(
            env,
            args[0],
            directory.as_mut_ptr(),
            directory.len(),
            &mut length
        ));
        let directory = &directory[..length];
        if directory.contains(&0) {
            return type_error(env, c"Capture directory contains a NUL");
        }
        let drive = length >= 3
            && directory[1] == b':' as u16
            && (directory[2] == b'\\' as u16 || directory[2] == b'/' as u16);
        let unc = length >= 3 && directory[0] == b'\\' as u16 && directory[1] == b'\\' as u16;
        if !drive && !unc {
            return type_error(env, c"Capture directory must be absolute");
        }
        let error = capture::arm(directory);
        if error != 0 {
            return hook_error(env, error);
        }
        unsigned(env, capture::status())
    }
}

unsafe extern "C" fn capture_status(env: Env, info: CallbackInfo) -> Value {
    unsafe {
        let mut args = [ptr::null_mut(); 1];
        if !arguments(env, info, &mut args) {
            return ptr::null_mut();
        }
        unsigned(env, capture::status())
    }
}

unsafe extern "C" fn cleanup(owner: *mut c_void) {
    unsafe {
        if managed::cleanup(owner).is_err() {
            fatal(c"Failed to detach native hooks during environment cleanup");
        }
    }
}

unsafe extern "C" fn initialize(env: Env, exports: Value) -> Value {
    unsafe {
        let api = napi::api();
        checked!((api.add_env_cleanup_hook)(env, cleanup, env));
        for (name, number) in [
            (c"apiVersion", NATIVE_API_VERSION),
            (c"pointerSize", size_of::<usize>() as u32),
        ] {
            let value = unsigned(env, number);
            if value.is_null() {
                return ptr::null_mut();
            }
            checked!((api.set_named_property)(env, exports, name.as_ptr(), value));
        }
        let payload_base = word(env, runtime::api().image_base);
        if payload_base.is_null() {
            return ptr::null_mut();
        }
        checked!((api.set_named_property)(
            env,
            exports,
            c"payloadBase".as_ptr(),
            payload_base
        ));
        let mut profile_hash = ptr::null_mut();
        let mut platform_version = ptr::null_mut();
        checked!((api.create_string_utf8)(
            env,
            PROFILE_SHA256.as_ptr().cast(),
            PROFILE_SHA256.len(),
            &mut profile_hash
        ));
        checked!((api.create_string_utf8)(
            env,
            PLATFORM_VERSION.as_ptr().cast(),
            PLATFORM_VERSION.len(),
            &mut platform_version
        ));
        let invoke_original = word(
            env,
            crate::dispatch::managed_invoke_original as *const () as u64,
        );
        if invoke_original.is_null() {
            return ptr::null_mut();
        }
        let release_properties = [
            Property::immutable(c"targetProfileSha256", profile_hash),
            Property::immutable(c"platformVersion", platform_version),
            Property::immutable(c"invokeOriginal", invoke_original),
        ];
        checked!((api.define_properties)(
            env,
            exports,
            release_properties.len(),
            release_properties.as_ptr()
        ));
        for (name, callback) in [
            (c"getExport", get_export as napi::Callback),
            (c"call", call),
            (c"attach", attach),
            (c"detach", detach),
            (c"dispose", dispose),
            (c"acquireOriginal", acquire_original),
            (c"releaseOriginal", release_original),
            (c"isDrained", is_drained),
            (c"armDlVmCapture", arm_capture),
            (c"dlVmCaptureStatus", capture_status),
        ] {
            let mut value = ptr::null_mut();
            checked!((api.create_function)(
                env,
                name.as_ptr(),
                AUTO_LENGTH,
                callback,
                ptr::null_mut(),
                &mut value
            ));
            checked!((api.set_named_property)(env, exports, name.as_ptr(), value));
        }
        exports
    }
}

unsafe extern "C" fn initialize_alias(env: Env, _: Value) -> Value {
    unsafe {
        let api = napi::api();
        let mut global = ptr::null_mut();
        let mut process = ptr::null_mut();
        let mut linked_binding = ptr::null_mut();
        let mut name = ptr::null_mut();
        let mut exports = ptr::null_mut();
        checked!((api.get_global)(env, &mut global));
        checked!((api.get_named_property)(
            env,
            global,
            c"process".as_ptr(),
            &mut process
        ));
        checked!((api.get_named_property)(
            env,
            process,
            c"_linkedBinding".as_ptr(),
            &mut linked_binding
        ));
        checked!((api.create_string_utf8)(
            env,
            c"phantomloader_native".as_ptr(),
            AUTO_LENGTH,
            &mut name
        ));
        checked!((api.call_function)(
            env,
            process,
            linked_binding,
            1,
            &name,
            &mut exports
        ));
        exports
    }
}

pub unsafe fn register(module: HMODULE) -> bool {
    unsafe {
        if REGISTERED.load(Ordering::Acquire) {
            return true;
        }
        if !napi::install(module) {
            return false;
        }
        (napi::api().module_register)(&raw mut MODULE);
        (napi::api().module_register)(&raw mut LEGACY_MODULE);
        REGISTERED.store(true, Ordering::Release);
        true
    }
}
