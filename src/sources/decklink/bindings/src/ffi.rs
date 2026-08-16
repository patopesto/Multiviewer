pub enum DecklinkSourceHandle {}
pub enum DecklinkDiscovery {}

#[cfg(any(target_os = "macos", target_os = "linux"))]
unsafe extern "C" {
    pub fn decklink_source_new(display_name: *const std::ffi::c_char) -> *mut DecklinkSourceHandle;
    pub fn decklink_source_free(s: *mut DecklinkSourceHandle);
    pub fn decklink_source_set_connection(s: *mut DecklinkSourceHandle, connection: u32);
    pub fn decklink_source_start(s: *mut DecklinkSourceHandle) -> bool;
    pub fn decklink_source_stop(s: *mut DecklinkSourceHandle);
    pub fn decklink_source_poll_frame(
        s: *mut DecklinkSourceHandle,
        out_rgba: *mut u8,
        out_size: usize,
        w: *mut i32,
        h: *mut i32,
        seq: *mut u64,
        fmt_out: *mut u32,
        nominal_fps_out: *mut f64,
    ) -> bool;

    pub fn decklink_discovery_new() -> *mut DecklinkDiscovery;
    pub fn decklink_discovery_free(d: *mut DecklinkDiscovery);
    pub fn decklink_discovery_count(d: *mut DecklinkDiscovery) -> i32;
    pub fn decklink_discovery_get(
        d: *mut DecklinkDiscovery,
        idx: i32,
        name: *mut std::ffi::c_char,
        name_len: usize,
        has_signal: *mut bool,
        connections: *mut u32,
    );
}
