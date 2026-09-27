use std::path::PathBuf;

/// Windows app-lifecycle hooks: optional console attachment.

pub fn init_console(show_console: bool) {
    if show_console {
        unsafe {
            attach_or_alloc_console();
        }
    }
}

pub fn setup_app() {}

pub fn on_window_created(_ctx: &egui::Context) {}

pub fn poll_open_file() -> Option<PathBuf> {
    return None;
}

unsafe fn attach_or_alloc_console() {
    type BOOL = i32;
    type DWORD = u32;
    const ATTACH_PARENT_PROCESS: DWORD = 0xFFFFFFFF;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(dwProcessId: DWORD) -> BOOL;
        fn AllocConsole() -> BOOL;
    }

    if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
        let _ = AllocConsole();
    }
}
