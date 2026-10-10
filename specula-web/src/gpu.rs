//! The device and canvas surface every page draws with.

use web_sys::HtmlCanvasElement;

pub struct Gpu {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    /// The API's name, for the FPS counter.
    pub backend: &'static str,
    /// The adapter as the browser names it, for the benchmark report.
    pub adapter: String,
}

pub async fn init(canvas: HtmlCanvasElement) -> Result<Gpu, String> {
    let (width, height) = (canvas.width(), canvas.height());
    // WebGPU where the browser has it, otherwise WebGL2, from one binary.
    let instance = wgpu::util::new_instance_with_webgpu_detection(
        wgpu::InstanceDescriptor::new_without_display_handle(),
    )
    .await;
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|error| format!("cannot create a surface: {error}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|error| format!("no graphics adapter: {error}"))?;
    let backend = match adapter.get_info().backend {
        wgpu::Backend::BrowserWebGpu => "WebGPU",
        wgpu::Backend::Gl => "WebGL",
        _ => "other",
    };
    crate::log(&format!("graphics backend: {backend}"));
    let info = adapter.get_info();
    let adapter_name = [info.name, info.driver, info.driver_info]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // Lets the observer page upload the game's DXT textures as they are;
    // without it they are decoded in WASM.
    let bc = adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC;
    crate::log(&format!(
        "BC textures: {}",
        if bc.is_empty() { "no" } else { "yes" }
    ));
    // Times the passes for the stats box; WebGL never has it.
    let timestamps = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    crate::log(&format!(
        "GPU timestamps: {}",
        if timestamps.is_empty() { "no" } else { "yes" }
    ));
    let features = bc | timestamps;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: features,
            // WebGL2 limits keep the fallback working; the resolution
            // limits come from the adapter so large canvases still fit.
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await
        .map_err(|error| format!("no graphics device: {error}"))?;

    let capabilities = surface.get_capabilities(&adapter);
    // Minimap colours and game textures are sRGB bytes already, and the
    // engine draws them without conversion; an sRGB target would encode
    // them a second time and wash them out.
    let format = capabilities
        .formats
        .iter()
        .copied()
        .find(|format| !format.is_srgb())
        .or_else(|| capabilities.formats.first().copied())
        .ok_or("the surface supports no format")?;
    let mut config = surface
        .get_default_config(&adapter, width.max(1), height.max(1))
        .ok_or("the surface is not supported by the adapter")?;
    config.format = format;
    surface.configure(&device, &config);
    Ok(Gpu {
        surface,
        device,
        queue,
        config,
        backend,
        adapter: adapter_name,
    })
}
