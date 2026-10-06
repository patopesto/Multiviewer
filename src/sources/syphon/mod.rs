mod discovery;
mod output;
mod source;

use std::ffi::{c_char, CStr};

use objc::runtime::{Class, Object};
use objc::{msg_send, sel, sel_impl};

pub use discovery::Discovery as SyphonDiscovery;
pub use discovery::format_syphon_label;
pub use source::{SyphonSource, SyphonSourceConfig};
pub use output::{SyphonOutput, SyphonOutputConfig};

// external crate re-exports
pub use syphon_core::ServerInfo as SyphonServerInfo;

/// Version string of the Syphon framework, from its bundle.
pub fn syphon_version() -> Option<String> {
    // syphon-core's `version()` calls the deprecated NSObject `+version` and
    // always yields an empty string, so read CFBundleShortVersionString instead.
    // SAFETY: Objective-C messages to the loaded Syphon framework and to
    // NSBundle/NSString; every returned object is used immediately.
    unsafe {
        let server_class = Class::get("SyphonServer")?;
        let bundle_class = Class::get("NSBundle")?;
        let string_class = Class::get("NSString")?;

        let bundle: *mut Object = msg_send![bundle_class, bundleForClass: server_class as *const Class as *mut Object];
        if bundle.is_null() {
            return None;
        }
        let key: *mut Object = msg_send![string_class, stringWithUTF8String: c"CFBundleShortVersionString".as_ptr()];
        if key.is_null() {
            return None;
        }
        let value: *mut Object = msg_send![bundle, objectForInfoDictionaryKey: key];
        if value.is_null() {
            return None;
        }
        let utf8: *const c_char = msg_send![value, UTF8String];
        if utf8.is_null() {
            return None;
        }
        let version = CStr::from_ptr(utf8).to_string_lossy().into_owned();
        if version.is_empty() {
            return None;
        }
        return Some(version);
    }
}