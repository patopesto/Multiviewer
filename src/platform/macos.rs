use std::ffi::{c_char, c_void, CStr, CString};
use std::os::raw::c_long;
use std::path::PathBuf;
use std::sync::Mutex;

use objc::runtime::{Class, Object, Sel};
use objc::{msg_send, sel, sel_impl};

// macOS app-lifecycle hooks: Finder file-open events and URL handling.
static QUEUE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn poll_open_file() -> Option<PathBuf> {
    return QUEUE.lock().unwrap().take();
}

fn queue_path(path: PathBuf) {
    *QUEUE.lock().unwrap() = Some(path);
}

// CoreFoundation types
type CFStringRef = *const c_void;
type CFNotificationCenterRef = *const c_void;
type CFNotificationCallback = extern "C" fn(
    center: CFNotificationCenterRef,
    observer: *mut c_void,
    name: CFStringRef,
    object: *const c_void,
    user_info: *const c_void,
);

// Carbon AppleEvent types
type AEEventClass = u32;
type AEEventID = u32;
type AEDesc = *const c_void;
type AEEventHandler = extern "C" fn(event: AEDesc, reply: *mut c_void, refcon: *mut c_void) -> i16;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        cStr: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFNotificationCenterGetLocalCenter() -> CFNotificationCenterRef;
    fn CFNotificationCenterAddObserver(
        center: CFNotificationCenterRef,
        observer: *const c_void,
        callback: CFNotificationCallback,
        name: CFStringRef,
        object: *const c_void,
        suspensionBehavior: u32,
    );
    fn CFRelease(cf: *const c_void);
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn AEInstallEventHandler(
        eventClass: AEEventClass,
        eventID: AEEventID,
        handler: AEEventHandler,
        refCon: *mut c_void,
        isMainThread: bool,
    ) -> i16;
    fn AEGetParamDesc(
        event: AEDesc,
        keyword: u32,
        desiredType: u32,
        result: *mut c_void,
    ) -> i16;
    fn AECountItems(theAEDescList: *const c_void, count: *mut c_long) -> i16;
    fn AEGetNthPtr(
        theAEDescList: *const c_void,
        index: c_long,
        desiredType: u32,
        theAEKeyword: *mut u32,
        typeCode: *mut u32,
        dataPtr: *mut c_void,
        maximumSize: c_long,
        actualSize: *mut c_long,
    ) -> i16;
    fn AEDisposeDesc(theAEDesc: *mut c_void) -> i16;
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;
const CF_NOTIFICATION_DELIVER_IMMEDIATELY: u32 = 1;

extern "C" fn on_will_finish_launching(
    _center: CFNotificationCenterRef,
    _observer: *mut c_void,
    _name: CFStringRef,
    _object: *const c_void,
    _user_info: *const c_void,
) {
    unsafe {
        inject_open_urls_method();
    }
}

unsafe fn inject_open_urls_method() {
    let app_cls = Class::get("NSApplication").expect("NSApplication class not found");
    let app: *mut Object = msg_send![app_cls, sharedApplication];
    let delegate: *mut Object = msg_send![app, delegate];
    if delegate.is_null() {
        return;
    }

    unsafe extern "C" {
        fn object_getClass(obj: *mut Object) -> *mut Class;
        fn class_addMethod(cls: *mut Class, name: Sel, imp: *const c_void, types: *const c_char) -> i32;
    }

    let cls = unsafe { object_getClass(delegate) };
    let sel = sel!(application:openURLs:);

    extern "C" fn handle_open_urls(
        _this: *mut Object,
        _cmd: Sel,
        _app: *mut Object,
        urls: *mut Object,
    ) {
        unsafe {
            let count: usize = msg_send![urls, count];
            if count > 0 {
                let url: *mut Object = msg_send![urls, objectAtIndex: 0];
                let path_str: *mut Object = msg_send![url, path];
                if !path_str.is_null() {
                    let bytes: *const c_char = msg_send![path_str, UTF8String];
                    if !bytes.is_null() && let Ok(cstr) = CStr::from_ptr(bytes).to_str() {
                        queue_path(PathBuf::from(cstr));
                    }
                }
            }
        }
    }

    let types = CString::new("v@:@@").unwrap();
    unsafe {
        class_addMethod(
            cls,
            sel,
            handle_open_urls as *const c_void,
            types.as_ptr(),
        );
    }
}

extern "C" fn handle_open_documents(
    event: AEDesc,
    _reply: *mut c_void,
    _refcon: *mut c_void,
) -> i16 {
    unsafe {
        let mut list: [u8; 80] = [0; 80];
        let err = AEGetParamDesc(
            event,
            u32::from_be_bytes(*b"----"),
            u32::from_be_bytes(*b"list"),
            list.as_mut_ptr() as *mut _,
        );
        if err != 0 {
            return 0;
        }

        let mut count: c_long = 0;
        if AECountItems(list.as_ptr() as *const _, &mut count) == 0 && count > 0 {
            let mut buf = [0u8; 4096];
            let mut keyword: u32 = 0;
            let mut type_code: u32 = 0;
            let mut actual: c_long = 0;
            let err = AEGetNthPtr(
                list.as_ptr() as *const _,
                1,
                u32::from_be_bytes(*b"furl"),
                &mut keyword,
                &mut type_code,
                buf.as_mut_ptr() as *mut _,
                buf.len() as c_long,
                &mut actual,
            );
            if err == 0
                && actual > 0
                && let Some(path) = file_url_to_path(&buf[..actual as usize])
            {
                queue_path(path);
            }
        }

        AEDisposeDesc(list.as_mut_ptr() as *mut _);
    }
    return 0;
}

fn file_url_to_path(bytes: &[u8]) -> Option<PathBuf> {
    let s = std::str::from_utf8(bytes).ok()?;
    let s = s.strip_prefix("file://")?;
    let path = percent_decode(s);
    return Some(PathBuf::from(path));
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 && let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
                continue;
            }
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }
    return result;
}

pub fn init_console(_show_console: bool) {}

pub fn setup_app() {
    unsafe {
        let name = CFStringCreateWithCString(
            std::ptr::null(),
            c"NSApplicationWillFinishLaunchingNotification".as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        );
        if name.is_null() {
            return;
        }

        CFNotificationCenterAddObserver(
            CFNotificationCenterGetLocalCenter(),
            std::ptr::null(),
            on_will_finish_launching,
            name,
            std::ptr::null(),
            CF_NOTIFICATION_DELIVER_IMMEDIATELY,
        );

        CFRelease(name);
    }
}

pub fn configure_wgpu(_options: &mut eframe::NativeOptions) {}

pub fn on_window_created(_ctx: &egui::Context) {
    unsafe {
        let err = AEInstallEventHandler(
            u32::from_be_bytes(*b"aevt"),
            u32::from_be_bytes(*b"odoc"),
            handle_open_documents,
            std::ptr::null_mut(),
            false,
        );
        if err != 0 {
            eprintln!("Failed to install AppleEvent handler: {}", err);
        }
    }
}
