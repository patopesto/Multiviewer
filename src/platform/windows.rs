use std::path::PathBuf;

/// Windows app-lifecycle hooks: optional console attachment.
pub fn init_console(show_console: bool) {
    if show_console {
        unsafe {
            attach_or_alloc_console();
        }
    }
}

unsafe fn attach_or_alloc_console() {
    type Bool = i32;
    type Dword = u32;
    const ATTACH_PARENT_PROCESS: Dword = 0xFFFFFFFF;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(dwProcessId: Dword) -> Bool;
        fn AllocConsole() -> Bool;
    }

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            let _ = AllocConsole();
        }
    }
}

pub fn setup_app() {}

/// Force wgpu to the D3D12 backend for Spout to work
pub fn configure_wgpu(options: &mut eframe::NativeOptions) {
    use eframe::egui_wgpu::WgpuSetup;
    if let WgpuSetup::CreateNew(create_new) = &mut options.wgpu_options.wgpu_setup {
        create_new.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    }
}

pub fn on_window_created(_ctx: &egui::Context) {}

pub fn poll_open_file() -> Option<PathBuf> {
    return None;
}
