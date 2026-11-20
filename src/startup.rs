use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui_wgpu::WgpuSetup;
use tracing_subscriber::prelude::*;
use wgpu::{Backend, Backends, DeviceDescriptor, Features, Instance, InstanceDescriptor, Limits, PowerPreference};

use crate::APP_NAME;
use crate::Cli;
use crate::engine::{EngineRuntime, StartupError, ViewerId};
use crate::platform;
use crate::session::Session;
use crate::ui;
use crate::ui::RemoteHost;

pub fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    // Wall clock from process start, so --exit-after includes startup.
    let deadline = cli.exit_after.map(|run| start + run);
    platform::init_console(cli.console);

    let trace_guard = init_tracing(cli.trace);
    platform::setup_process();

    let session = Session::load();
    let startup_path = cli.project.or(session.last_project);

    if cli.headless {
        return run_headless(startup_path, trace_guard, deadline);
    }

    platform::setup_app();
    return run_native(startup_path, trace_guard, deadline);
}

fn run_native(
    startup_path: Option<PathBuf>,
    trace_guard: Option<tracing_chrome::FlushGuard>,
    deadline: Option<Instant>,
) -> Result<(), Box<dyn std::error::Error>> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/appicon/AppIcon.png")))
        .expect("failed to decode app icon");
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1600.0, 900.0]).with_icon(Arc::new(icon)),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };
    install_device_descriptor(&mut options);
    platform::configure_wgpu(&mut options);

    println!("Hello from Multiviewer");
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            platform::on_window_created(&cc.egui_ctx);
            log_wgpu_backend(cc);
            install_wgpu_error_handler(cc);
            return Ok(Box::new(ui::App::new(cc, startup_path, trace_guard, deadline)));
        }),
    )?;
    return Ok(());
}

fn run_headless(
    startup_path: Option<PathBuf>,
    trace_guard: Option<tracing_chrome::FlushGuard>,
    deadline: Option<Instant>,
) -> Result<(), Box<dyn std::error::Error>> {
    let gpu = create_headless_device(platform::headless_backends()).map_err(std::io::Error::other)?;
    let info = gpu.adapter.get_info();
    tracing::info!("headless wgpu backend {:?}, adapter '{}' ({:?})", info.backend, info.name, info.device_type);

    let mut runtime = EngineRuntime::start(startup_path, gpu.device.clone(), gpu.queue.clone(), wgpu::TextureFormat::Bgra8Unorm, false, None)
        .map_err(std::io::Error::other)?;

    match &runtime.startup_error {
        Some(StartupError::Missing(message)) => tracing::warn!("{message}"),
        Some(StartupError::Open(message)) => {
            return Err(std::io::Error::other(message.clone()).into());
        }
        None => {}
    }
    if let Some(warning) = &runtime.startup_warning {
        tracing::warn!("{warning}");
    }

    let mut remote = RemoteHost::new(
        runtime.engine.clone(),
        runtime.runner.snapshot_slot(),
        runtime.runner.commands(),
        runtime.runner.focus_handle(ViewerId::Remote),
    );
    remote.sync();

    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break;
        }
        remote.sync();
        std::thread::sleep(Duration::from_millis(250));
    }

    drop(remote);
    runtime.runner.stop();
    runtime.engine.lock().unwrap().log_run_totals();
    drop(trace_guard);
    return Ok(());
}

/// Ask for the texture storage feature `vmx` checks to pick its decode path:
/// without it the decoder computes into a buffer and copies it into the frame.
fn device_descriptor(adapter: &wgpu::Adapter) -> DeviceDescriptor<'static> {
    let mut features = Features::empty();
    if adapter.features().contains(Features::BGRA8UNORM_STORAGE) {
        features |= Features::BGRA8UNORM_STORAGE;
    }
    // Optional: the canvas pass samples its own duration with timestamp
    // queries, and GL/GLES adapters must still start without them.
    if adapter.features().contains(Features::TIMESTAMP_QUERY) {
        features |= Features::TIMESTAMP_QUERY;
    }
    let base_limits = if adapter.get_info().backend == Backend::Gl {
        Limits::downlevel_webgl2_defaults()
    } else {
        Limits::default()
    };
    return DeviceDescriptor {
        label: Some("Multiviewer wgpu device"),
        required_features: features,
        required_limits: Limits {
            max_texture_dimension_2d: 8192,
            ..base_limits
        },
        ..Default::default()
    };
}

fn install_device_descriptor(options: &mut eframe::NativeOptions) {
    let WgpuSetup::CreateNew(create_new) = &mut options.wgpu_options.wgpu_setup else {
        return;
    };
    create_new.device_descriptor = Arc::new(|adapter: &wgpu::Adapter| device_descriptor(adapter));
}

struct HeadlessGpu {
    /// The instance and adapter must outlive the device they created.
    _instance: Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn create_headless_device(backends: Backends) -> Result<HeadlessGpu, String> {
    let instance = Instance::new(InstanceDescriptor {
        backends,
        ..InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .map_err(|e| format!("no headless wgpu adapter: {e}"))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&device_descriptor(&adapter))).map_err(|e| format!("headless wgpu device: {e}"))?;
    return Ok(HeadlessGpu {
        _instance: instance,
        adapter,
        device,
        queue,
    });
}

/// Chrome/Perfetto trace export when `--trace <file>.json` is passed.
fn init_tracing(trace_path: Option<PathBuf>) -> Option<tracing_chrome::FlushGuard> {
    // Console logging, controlled by RUST_LOG
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "Multiviewer=info".into());
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(filter);

    let Some(trace_path) = trace_path else {
        tracing_subscriber::registry().with(fmt_layer).init();
        return None;
    };
    let writer = match std::fs::File::create(&trace_path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("cannot create trace file {}: {e}", trace_path.display());
            tracing_subscriber::registry().with(fmt_layer).init();
            return None;
        }
    };
    let (chrome_layer, guard) = tracing_chrome::ChromeLayerBuilder::new().writer(writer).include_args(true).build();

    // Filter out external lib traces
    let chrome_layer =
        chrome_layer.with_filter(tracing_subscriber::filter::Targets::new().with_target(env!("CARGO_CRATE_NAME"), tracing::Level::DEBUG));
    tracing_subscriber::registry().with(fmt_layer).with(chrome_layer).init();
    tracing::info!("writing trace to {}", trace_path.display());
    return Some(guard);
}

fn log_wgpu_backend(cc: &eframe::CreationContext<'_>) {
    match &cc.wgpu_render_state {
        Some(render_state) => {
            let info = render_state.adapter.get_info();
            tracing::info!("wgpu backend {:?}, adapter '{}' ({:?})", info.backend, info.name, info.device_type);
        }
        None => tracing::info!("wgpu renderer not in use"),
    }
}

/// wgpu panics on every uncaptured validation error by default, which aborts the
/// process when it happens inside winit's event handlers. The vendored
/// `wgpu-core` accepts a non-empty queue during `Surface::configure`; keep the
/// handler for OMT and any other off-thread submitter.
fn install_wgpu_error_handler(cc: &eframe::CreationContext<'_>) {
    let Some(render_state) = &cc.wgpu_render_state else {
        return;
    };
    render_state.device.on_uncaptured_error(Arc::new(|error| {
        tracing::error!("wgpu error: {error}");
    }));
}
