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
    pub fn decklink_source_set_gpu_mode(s: *mut DecklinkSourceHandle, enable: bool);
    pub fn decklink_source_is_bgra(s: *mut DecklinkSourceHandle) -> bool;
    pub fn decklink_source_poll_cv_frame(
        s: *mut DecklinkSourceHandle,
        cv_out: *mut *mut std::ffi::c_void,
        w: *mut i32,
        h: *mut i32,
        seq: *mut u64,
        fmt_out: *mut u32,
        nominal_fps_out: *mut f64,
    ) -> bool;
    pub fn decklink_source_take_cpu_fallback(s: *mut DecklinkSourceHandle) -> bool;
    pub fn decklink_source_frame_generation(s: *mut DecklinkSourceHandle) -> u64;
    pub fn decklink_source_wait_frame(s: *mut DecklinkSourceHandle, seen: u64, timeout_ms: i32);
    pub fn decklink_source_set_capture_buffers(
        s: *mut DecklinkSourceHandle,
        pointers: *mut *mut u8,
        sizes: *const u32,
        count: u32,
    ) -> bool;
    pub fn decklink_source_pinned_available(s: *mut DecklinkSourceHandle) -> bool;
    pub fn decklink_source_enable_pinned(s: *mut DecklinkSourceHandle) -> bool;
    pub fn decklink_source_disable_pinned(s: *mut DecklinkSourceHandle) -> bool;
    pub fn decklink_source_poll_pinned_frame(
        s: *mut DecklinkSourceHandle,
        index: *mut i32,
        w: *mut i32,
        h: *mut i32,
        seq: *mut u64,
        fmt_out: *mut u32,
        nominal_fps_out: *mut f64,
    ) -> bool;
    pub fn decklink_source_return_buffer(s: *mut DecklinkSourceHandle, index: i32);
    pub fn decklink_source_frame_info(
        s: *mut DecklinkSourceHandle,
        w: *mut u32,
        h: *mut u32,
        fmt_out: *mut u32,
    );
    pub fn decklink_cv_frame_release(cv: *mut std::ffi::c_void);
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
        token: u64,
        token_queued: *mut bool,
    ) -> bool;
    pub fn decklink_output_present_cv(
        o: *mut DecklinkOutputHandle,
        cv: *mut std::ffi::c_void,
        token: u64,
    ) -> bool;
    pub fn decklink_cv_buffer_new(width: i32, height: i32) -> *mut std::ffi::c_void;
    pub fn decklink_cv_buffer_iosurface(cv: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    pub fn decklink_cv_buffer_release(cv: *mut std::ffi::c_void);
    pub fn decklink_output_take_completed(
        o: *mut DecklinkOutputHandle,
        token: *mut u64,
    ) -> bool;
    pub fn decklink_output_completion_available(o: *mut DecklinkOutputHandle) -> bool;
    pub fn decklink_output_pixel_format(o: *mut DecklinkOutputHandle) -> u32;
}
