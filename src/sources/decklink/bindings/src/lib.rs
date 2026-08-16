#[allow(
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    dead_code
)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod sdk {
    include!(concat!(env!("OUT_DIR"), "/decklink_sdk_bindings.rs"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use sdk::*;

pub mod ffi;
pub mod types;

pub use ffi::*;
pub use types::*;
