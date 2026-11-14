// SPDX-FileCopyrightText: 2026 movblock
// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::runtime;
use core::{
    ffi::{c_char, c_void},
    mem::MaybeUninit,
    ptr,
};
use windows_sys::Win32::Foundation::HMODULE;

pub type Env = *mut c_void;
pub type Value = *mut c_void;
pub type CallbackInfo = *mut c_void;
pub type Status = i32;
pub type Callback = unsafe extern "C" fn(Env, CallbackInfo) -> Value;
pub type Register = unsafe extern "C" fn(Env, Value) -> Value;
pub const OK: Status = 0;
pub const AUTO_LENGTH: usize = usize::MAX;
pub const ENUMERABLE: u32 = 1 << 1;
const NAPI_MODULE_VERSION: i32 = 1;
// node::ModuleFlags::kLinked registers a built-in binding before node::Init.
const NODE_MODULE_LINKED_FLAG: u32 = 2;

#[repr(C)]
pub struct Module {
    version: i32,
    flags: u32,
    filename: *const c_char,
    register: Register,
    name: *const c_char,
    private: *mut c_void,
    reserved: [*mut c_void; 4],
}

impl Module {
    pub const fn linked(name: &'static core::ffi::CStr, register: Register) -> Self {
        Self {
            version: NAPI_MODULE_VERSION,
            flags: NODE_MODULE_LINKED_FLAG,
            filename: c"phantomloader_payload".as_ptr(),
            register,
            name: name.as_ptr(),
            private: ptr::null_mut(),
            reserved: [ptr::null_mut(); 4],
        }
    }
}

#[repr(C)]
pub struct Property {
    pub utf8_name: *const c_char,
    pub name: Value,
    pub method: Option<Callback>,
    pub getter: Option<Callback>,
    pub setter: Option<Callback>,
    pub value: Value,
    pub attributes: u32,
    pub data: *mut c_void,
}

impl Property {
    pub fn immutable(name: &'static core::ffi::CStr, value: Value) -> Self {
        Self {
            utf8_name: name.as_ptr(),
            name: ptr::null_mut(),
            method: None,
            getter: None,
            setter: None,
            value,
            attributes: ENUMERABLE,
            data: ptr::null_mut(),
        }
    }
}

macro_rules! api_table {
    ($( $field:ident : $name:literal => $signature:ty; )+) => {
        pub struct Api { $(pub $field: $signature,)+ }

        impl Api {
            pub unsafe fn resolve(module: HMODULE) -> Option<Self> {
                unsafe {
                    Some(Self {
                        $($field: core::mem::transmute::<unsafe extern "system" fn() -> isize, $signature>(
                            runtime::resolve(module, $name)?,
                        ),)+
                    })
                }
            }
        }
    }
}

api_table! {
    module_register: c"napi_module_register" => unsafe extern "C" fn(*mut Module);
    create_uint32: c"napi_create_uint32" => unsafe extern "C" fn(Env, u32, *mut Value) -> Status;
    create_string_utf8: c"napi_create_string_utf8" => unsafe extern "C" fn(Env, *const c_char, usize, *mut Value) -> Status;
    define_properties: c"napi_define_properties" => unsafe extern "C" fn(Env, Value, usize, *const Property) -> Status;
    create_bigint_uint64: c"napi_create_bigint_uint64" => unsafe extern "C" fn(Env, u64, *mut Value) -> Status;
    get_value_bigint_uint64: c"napi_get_value_bigint_uint64" => unsafe extern "C" fn(Env, Value, *mut u64, *mut bool) -> Status;
    get_value_string_utf16: c"napi_get_value_string_utf16" => unsafe extern "C" fn(Env, Value, *mut u16, usize, *mut usize) -> Status;
    get_value_string_utf8: c"napi_get_value_string_utf8" => unsafe extern "C" fn(Env, Value, *mut c_char, usize, *mut usize) -> Status;
    set_named_property: c"napi_set_named_property" => unsafe extern "C" fn(Env, Value, *const c_char, Value) -> Status;
    create_function: c"napi_create_function" => unsafe extern "C" fn(Env, *const c_char, usize, Callback, *mut c_void, *mut Value) -> Status;
    get_cb_info: c"napi_get_cb_info" => unsafe extern "C" fn(Env, CallbackInfo, *mut usize, *mut Value, *mut Value, *mut *mut c_void) -> Status;
    throw_type_error: c"napi_throw_type_error" => unsafe extern "C" fn(Env, *const c_char, *const c_char) -> Status;
    throw_range_error: c"napi_throw_range_error" => unsafe extern "C" fn(Env, *const c_char, *const c_char) -> Status;
    throw_error: c"napi_throw_error" => unsafe extern "C" fn(Env, *const c_char, *const c_char) -> Status;
    get_array_length: c"napi_get_array_length" => unsafe extern "C" fn(Env, Value, *mut u32) -> Status;
    is_array: c"napi_is_array" => unsafe extern "C" fn(Env, Value, *mut bool) -> Status;
    get_element: c"napi_get_element" => unsafe extern "C" fn(Env, Value, u32, *mut Value) -> Status;
    create_object: c"napi_create_object" => unsafe extern "C" fn(Env, *mut Value) -> Status;
    get_global: c"napi_get_global" => unsafe extern "C" fn(Env, *mut Value) -> Status;
    get_named_property: c"napi_get_named_property" => unsafe extern "C" fn(Env, Value, *const c_char, *mut Value) -> Status;
    call_function: c"napi_call_function" => unsafe extern "C" fn(Env, Value, Value, usize, *const Value, *mut Value) -> Status;
    get_undefined: c"napi_get_undefined" => unsafe extern "C" fn(Env, *mut Value) -> Status;
    get_boolean: c"napi_get_boolean" => unsafe extern "C" fn(Env, bool, *mut Value) -> Status;
    get_value_double: c"napi_get_value_double" => unsafe extern "C" fn(Env, Value, *mut f64) -> Status;
    add_env_cleanup_hook: c"napi_add_env_cleanup_hook" => unsafe extern "C" fn(Env, unsafe extern "C" fn(*mut c_void), *mut c_void) -> Status;
    fatal_error: c"napi_fatal_error" => unsafe extern "C" fn(*const c_char, usize, *const c_char, usize) -> !;
}

static mut API: MaybeUninit<Api> = MaybeUninit::uninit();

pub unsafe fn install(module: HMODULE) -> bool {
    unsafe {
        let Some(table) = Api::resolve(module) else {
            return false;
        };
        (&raw mut API).write(MaybeUninit::new(table));
        true
    }
}

pub fn api() -> &'static Api {
    unsafe { &*(&raw const API).cast::<Api>() }
}
