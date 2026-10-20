pub enum DecklinkSourceHandle {}
pub enum DecklinkDiscovery {}
pub enum DecklinkOutputHandle {}

unsafe extern "C" {
    // API information
    pub fn decklink_api_version(buf: *mut std::ffi::c_char, buf_len: i32) -> i32;

    // Discovery (one iterator pass for both input and output ports)
    pub fn decklink_discovery_new() -> *mut DecklinkDiscovery;
    pub fn decklink_discovery_free(d: *mut DecklinkDiscovery);
    pub fn decklink_discovery_input_count(d: *mut DecklinkDiscovery) -> i32;
    pub fn decklink_discovery_input_get(
        d: *mut DecklinkDiscovery,
        idx: i32,
        name: *mut std::ffi::c_char,
        name_len: usize,
        has_signal: *mut bool,
        connections: *mut u32,
    );
    pub fn decklink_discovery_output_count(d: *mut DecklinkDiscovery) -> i32;
    pub fn decklink_discovery_output_get(
        d: *mut DecklinkDiscovery,
        idx: i32,
        name: *mut std::ffi::c_char,
        name_len: usize,
        mode_count: *mut i32,
    );
    pub fn decklink_discovery_output_mode(
        d: *mut DecklinkDiscovery,
        idx: i32,
        mode_idx: i32,
        mode_name: *mut std::ffi::c_char,
        mode_name_len: usize,
        mode_id: *mut u32,
        w: *mut i32,
        h: *mut i32,
        fps: *mut f64,
    );

    // Input
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

    // Output
    pub fn decklink_output_new(display_name: *const std::ffi::c_char) -> *mut DecklinkOutputHandle;
    pub fn decklink_output_free(o: *mut DecklinkOutputHandle);
    pub fn decklink_output_start(o: *mut DecklinkOutputHandle, mode_id: u32) -> bool;
    pub fn decklink_output_stop(o: *mut DecklinkOutputHandle);
    pub fn decklink_output_present_frame(
        o: *mut DecklinkOutputHandle,
        bgra: *const u8,
        width: i32,
        height: i32,
        row_bytes: i32,
    ) -> bool;
}
