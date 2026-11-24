//! Os pedaços de `mpv/client.h` e `mpv/render.h` que o player usa. Os enums do C são `int`.
use std::ffi::{c_char, c_double, c_int, c_ulong, c_void};

#[repr(C)]
pub struct Handle { _private: [u8; 0] }

#[repr(C)]
pub struct RenderContext { _private: [u8; 0] }

#[cfg(test)]
pub const FORMAT_NONE: c_int = 0;
pub const FORMAT_FLAG: c_int = 3;
pub const FORMAT_INT64: c_int = 4;
pub const FORMAT_DOUBLE: c_int = 5;

pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_COMMAND_REPLY: c_int = 5;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_PROPERTY_CHANGE: c_int = 22;

pub const END_FILE_REASON_ERROR: c_int = 4;

#[repr(C)]
pub struct Event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct EventProperty {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

/// Só os dois primeiros campos são lidos; o resto da struct não importa por ponteiro.
#[repr(C)]
pub struct EventEndFile {
    pub reason: c_int,
    pub error: c_int,
}

pub const RENDER_PARAM_INVALID: c_int = 0;
pub const RENDER_PARAM_API_TYPE: c_int = 1;
pub const RENDER_PARAM_SW_SIZE: c_int = 17;
pub const RENDER_PARAM_SW_FORMAT: c_int = 18;
pub const RENDER_PARAM_SW_STRIDE: c_int = 19;
pub const RENDER_PARAM_SW_POINTER: c_int = 20;

pub const RENDER_API_TYPE_SW: &[u8] = b"sw\0";
pub const RENDER_UPDATE_FRAME: u64 = 1;

#[repr(C)]
pub struct RenderParam {
    pub kind: c_int,
    pub data: *mut c_void,
}

pub type UpdateFn = unsafe extern "C" fn(*mut c_void);

/// Funções resolvidas da biblioteca carregada; valem enquanto o `Library` que as guarda existir.
pub struct Api {
    pub client_api_version: unsafe extern "C" fn() -> c_ulong,
    pub create: unsafe extern "C" fn() -> *mut Handle,
    pub initialize: unsafe extern "C" fn(*mut Handle) -> c_int,
    pub terminate_destroy: unsafe extern "C" fn(*mut Handle),
    pub set_option_string: unsafe extern "C" fn(*mut Handle, *const c_char, *const c_char) -> c_int,
    pub command_async: unsafe extern "C" fn(*mut Handle, u64, *mut *const c_char) -> c_int,
    pub observe_property: unsafe extern "C" fn(*mut Handle, u64, *const c_char, c_int) -> c_int,
    pub wait_event: unsafe extern "C" fn(*mut Handle, c_double) -> *mut Event,
    pub wakeup: unsafe extern "C" fn(*mut Handle),
    pub error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    pub render_context_create: unsafe extern "C" fn(*mut *mut RenderContext, *mut Handle, *mut RenderParam) -> c_int,
    pub render_context_set_update_callback: unsafe extern "C" fn(*mut RenderContext, Option<UpdateFn>, *mut c_void),
    pub render_context_update: unsafe extern "C" fn(*mut RenderContext) -> u64,
    pub render_context_render: unsafe extern "C" fn(*mut RenderContext, *mut RenderParam) -> c_int,
    pub render_context_free: unsafe extern "C" fn(*mut RenderContext),
}

/// Versão `major` do `mpv_client_api_version` (`MPV_MAKE_VERSION`: maior nos 16 bits de cima).
pub fn api_major(version: c_ulong) -> c_ulong { version >> 16 }
