pub enum DecklinkSourceHandle {}
pub enum DecklinkSourceDiscovery {}
pub enum DecklinkOutputDiscovery {}
pub enum DecklinkOutputHandle {}

unsafe extern "C" {
    // Input
    pub fn decklink_source_discovery_new() -> *mut DecklinkSourceDiscovery;
    pub fn decklink_source_discovery_free(d: *mut DecklinkSourceDiscovery);
    pub fn decklink_source_discovery_count(d: *mut DecklinkSourceDiscovery) -> i32;
    pub fn decklink_source_discovery_get(
        d: *mut DecklinkSourceDiscovery,
        idx: i32,
        name: *mut std::ffi::c_char,
        name_len: usize,
        has_signal: *mut bool,
        connections: *mut u32,
    );

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
    pub fn decklink_output_discovery_new() -> *mut DecklinkOutputDiscovery;
    pub fn decklink_output_discovery_free(d: *mut DecklinkOutputDiscovery);
    pub fn decklink_output_discovery_count(d: *mut DecklinkOutputDiscovery) -> i32;
    pub fn decklink_output_discovery_get(
        d: *mut DecklinkOutputDiscovery,
        idx: i32,
        name: *mut std::ffi::c_char,
        name_len: usize,
        mode_count: *mut i32,
    );
    pub fn decklink_output_discovery_get_mode(
        d: *mut DecklinkOutputDiscovery,
        idx: i32,
        mode_idx: i32,
        mode_name: *mut std::ffi::c_char,
        mode_name_len: usize,
        mode_id: *mut u32,
        w: *mut i32,
        h: *mut i32,
        fps: *mut f64,
    );

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
