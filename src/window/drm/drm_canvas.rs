//! DRM Canvas for headless rendering without a window manager.

use crate::context::Context;
use crate::resource::OffscreenBuffers;
use crate::window::canvas_traits::{RenderCanvas, ScreenshotCanvas};
use crate::window::canvas_utils::{pixel_reading, texture_utils};
use std::error::Error;
use std::fmt;

#[cfg(feature = "drm")]
use super::card::Card;
#[cfg(feature = "drm")]
use super::display_thread::{BufferPool, DisplayCommand, DisplayThread, DisplayThreadConfig};
#[cfg(feature = "drm")]
use drm::buffer::DrmFourcc;
#[cfg(feature = "drm")]
use drm::control::{connector, crtc, Device as ControlDevice, Mode, ResourceHandles};

/// Error type for DRM canvas operations.
#[cfg(feature = "drm")]
#[derive(Debug)]
pub enum DrmCanvasError {
    DeviceRequest(String),
    Init(String),
    DrmError(String),
    ModesetError(String),
    PageFlipError(String),
    IoError(std::io::Error),
    NoConnectedDisplay,
}

impl fmt::Display for DrmCanvasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DrmCanvasError::DeviceRequest(msg) => write!(f, "Failed to request device: {}", msg),
            DrmCanvasError::Init(msg) => write!(f, "Initialization error: {}", msg),
            DrmCanvasError::DrmError(msg) => write!(f, "DRM error: {}", msg),
            DrmCanvasError::ModesetError(msg) => write!(f, "Display configuration error: {}", msg),
            DrmCanvasError::PageFlipError(msg) => write!(f, "Page flip error: {}", msg),
            DrmCanvasError::IoError(e) => write!(f, "I/O error: {}", e),
            DrmCanvasError::NoConnectedDisplay => write!(f, "No connected display found"),
        }
    }
}

impl Error for DrmCanvasError {}

impl From<std::io::Error> for DrmCanvasError {
    fn from(err: std::io::Error) -> Self {
        DrmCanvasError::IoError(err)
    }
}

/// Rendering mode for DrmCanvas
enum RenderMode {
    /// Offscreen rendering only (screenshots/recording)
    Offscreen,
    /// Display output via DRM/KMS
    #[cfg(feature = "drm")]
    Display(Box<DrmDisplayState>),
}

/// A canvas for headless rendering using offscreen buffers.
///
/// This canvas initializes wgpu without winit, allowing rendering
/// on systems without a window manager (e.g., console-only Raspberry Pi).
pub struct DrmCanvas {
    /// Offscreen render target (includes depth texture)
    offscreen_buffers: OffscreenBuffers,
    /// Surface configuration (dimensions and format)
    surface_config: DrmSurfaceConfig,
    /// Rendering mode (offscreen or display)
    mode: RenderMode,
    /// Texture for reading back pixels (for screenshots)
    readback_texture: wgpu::Texture,
}

/// Configuration for the DRM surface (mimics wgpu::SurfaceConfiguration).
struct DrmSurfaceConfig {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

/// Display configuration discovered from hardware
#[cfg(feature = "drm")]
struct DisplayConfig {
    connector: connector::Handle,
    crtc: crtc::Handle,
    mode: Mode,
    width: u32,
    height: u32,
}

/// Format compatibility information
#[cfg(feature = "drm")]
struct FormatInfo {
    wgpu_format: wgpu::TextureFormat,
    drm_format: DrmFourcc,
}

/// DRM display state for actual screen output
#[cfg(feature = "drm")]
struct DrmDisplayState {
    /// Async display thread for non-blocking presentation
    display_thread: DisplayThread,
    /// Buffer pool for recycling pixel buffers
    buffer_pool: BufferPool,
}

impl DrmCanvas {
    /// Creates a new DRM canvas for offscreen-only rendering (no display output).
    ///
    /// This mode is suitable for:
    /// - Screenshot/recording without display hardware
    /// - Server-side rendering
    /// - Testing without a monitor
    ///
    /// No DRM device is required for this mode.
    ///
    /// # Arguments
    /// * `width` - Width of the render target
    /// * `height` - Height of the render target
    ///
    /// # Returns
    /// A new DrmCanvas ready for offscreen rendering
    ///
    /// # Errors
    /// Returns an error if wgpu initialization fails
    pub async fn new(width: u32, height: u32) -> Result<Self, DrmCanvasError> {
        // Ensure minimum dimensions
        let width = width.max(1);
        let height = height.max(1);

        // Initialize wgpu without winit
        Self::init_wgpu_headless().await?;

        let format = wgpu::TextureFormat::Bgra8Unorm;

        let surface_config = DrmSurfaceConfig {
            width,
            height,
            format,
        };

        // Create offscreen render target (includes depth texture)
        let offscreen_buffers = OffscreenBuffers::new(width, height, format, true);

        let ctxt = Context::get();
        let readback_texture = Self::create_readback_texture(&ctxt.device, width, height, format);

        Ok(Self {
            offscreen_buffers,
            surface_config,
            mode: RenderMode::Offscreen,
            readback_texture,
        })
    }

    /// Creates a new DRM canvas with display output.
    ///
    /// This constructor initializes GBM and sets up the display pipeline for
    /// actual screen output via KMS/DRM.
    ///
    /// # Arguments
    /// * `device_path` - Path to DRM device (e.g., "/dev/dri/card0")
    ///
    /// # Returns
    /// A new DrmCanvas ready for display rendering
    ///
    /// # Errors
    /// Returns an error if:
    /// - DRM device cannot be opened
    /// - No connected display is found
    /// - GBM initialization fails
    /// - wgpu initialization fails
    #[cfg(feature = "drm")]
    pub async fn new_with_display(device_path: &str) -> Result<Self, DrmCanvasError> {
        log::info!("Creating DRM canvas with display output: {}", device_path);

        // Step 1: Open DRM device (for querying)
        let card = Card::open(device_path)?;
        log::info!("Opened DRM device: {}", device_path);

        // Step 2: Query display resources
        let display_config = Self::query_display_resources(&card)?;
        log::info!(
            "Display configuration: {}x{} @ {}Hz",
            display_config.width,
            display_config.height,
            display_config.mode.vrefresh()
        );

        // Step 3: Choose compatible formats
        let format_info = Self::choose_formats();
        log::info!(
            "Format selection - wgpu: {:?}, drm: {:?}",
            format_info.wgpu_format,
            format_info.drm_format
        );

        // Step 4: Initialize wgpu (headless)
        Self::init_wgpu_headless().await?;

        // Step 5: Create offscreen buffers for rendering (includes depth texture)
        let offscreen_buffers = OffscreenBuffers::new(
            display_config.width,
            display_config.height,
            format_info.wgpu_format,
            true,
        );

        // Create readback texture for screenshots
        let ctxt = Context::get();
        let readback_texture = Self::create_readback_texture(
            &ctxt.device,
            display_config.width,
            display_config.height,
            format_info.wgpu_format,
        );

        // Step 6: Create buffer pool for pixel data (triple buffering)
        log::info!("Creating buffer pool for triple buffering...");
        let buffer_size = (display_config.width * display_config.height * 4) as usize;
        let buffer_pool = BufferPool::new(3, buffer_size);

        // Step 7: Create display thread for async presentation
        log::info!("Starting async display thread...");
        let display_thread = DisplayThread::new(
            card,
            DisplayThreadConfig {
                connector: display_config.connector,
                crtc: display_config.crtc,
                mode: display_config.mode,
            },
            buffer_pool.recycler(),
            display_config.width,
            display_config.height,
        );

        // Step 8: Create display state
        let display_state = DrmDisplayState {
            display_thread,
            buffer_pool,
        };

        log::info!("DRM canvas with display created successfully");
        Ok(Self {
            offscreen_buffers,
            surface_config: DrmSurfaceConfig {
                width: display_config.width,
                height: display_config.height,
                format: format_info.wgpu_format,
            },
            mode: RenderMode::Display(Box::new(display_state)),
            readback_texture,
        })
    }

    /// Initialize wgpu without a window (headless mode).
    async fn init_wgpu_headless() -> Result<(), DrmCanvasError> {
        // Skip initialization if already done (multi-window case)
        if Context::is_initialized() {
            log::info!("wgpu context already initialized, reusing");
            return Ok(());
        }

        log::info!("Initializing wgpu for headless DRM rendering");

        // Create wgpu instance with primary backends (Vulkan on Linux)
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        // Request adapter without a surface (headless)
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None, // No surface for headless
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| {
                DrmCanvasError::DeviceRequest(format!("Failed to request adapter: {:?}", e))
            })?;

        log::info!("Adapter info: {:?}", adapter.get_info());

        // Use the adapter's supported limits to ensure compatibility with lower-end hardware
        // like Raspberry Pi (V3D GPU supports max_color_attachments=4, not 8)
        let adapter_limits = adapter.limits();
        log::debug!("Adapter limits: {:?}", adapter_limits);

        // Request device and queue
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("drm_device"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter_limits,
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::default(),
            })
            .await
            .map_err(|e: wgpu::RequestDeviceError| DrmCanvasError::DeviceRequest(e.to_string()))?;

        // Choose surface format (standard for offscreen rendering)
        let surface_format = wgpu::TextureFormat::Bgra8Unorm;

        // Initialize global context
        Context::init(instance, device, queue, adapter, surface_format);
        Context::increment_window_count();

        log::info!("wgpu initialized successfully for DRM");

        Ok(())
    }

    /// Gets the current texture for rendering.
    ///
    /// For DRM canvas, this returns a wrapper around the offscreen color texture.
    pub fn get_current_texture(&self) -> Result<DrmSurfaceTexture<'_>, DrmCanvasError> {
        Ok(DrmSurfaceTexture {
            texture: &self.offscreen_buffers.color_texture,
        })
    }

    /// Returns the depth texture view from offscreen buffers.
    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.offscreen_buffers.depth_view
    }

    /// Presents the rendered frame.
    ///
    /// For offscreen rendering, this is a no-op. For display mode,
    /// this triggers a page flip to show the rendered frame.
    ///
    /// # Arguments
    /// * `_frame` - The surface texture (unused in DRM, kept for API compatibility)
    pub fn present(&self, _frame: DrmSurfaceTexture) -> Result<(), DrmCanvasError> {
        match &self.mode {
            RenderMode::Offscreen => {
                // No-op for offscreen rendering
                Ok(())
            }
            #[cfg(feature = "drm")]
            RenderMode::Display(display) => {
                log::debug!("Present: starting frame presentation");

                let width = self.surface_config.width;
                let height = self.surface_config.height;

                // Step 1: Get a buffer from the pool (blocks if none available)
                let mut pixel_buffer = display.buffer_pool.try_get_buffer().unwrap_or_else(|| {
                    log::warn!("No buffer available, allocating new buffer");
                    vec![0u8; (width * height * 4) as usize]
                });

                // Step 2: Read GPU texture into the CPU buffer
                Self::read_texture_to_buffer(
                    &self.offscreen_buffers.color_texture,
                    &mut pixel_buffer,
                    width,
                    height,
                )?;
                log::debug!("Present: read GPU texture to CPU buffer");

                // Step 3: Send pixel data to display thread (non-blocking!)
                let command = DisplayCommand {
                    pixel_data: pixel_buffer,
                    width,
                    height,
                };

                if let Err(e) = display.display_thread.send_frame(command) {
                    log::error!("Failed to send frame to display thread: {}", e);
                    return Err(DrmCanvasError::PageFlipError(format!(
                        "Failed to send frame to display thread: {}",
                        e
                    )));
                }

                log::trace!("Frame queued for async display - main thread continues");
                log::debug!("Present: complete");
                Ok(())
            }
        }
    }

    /// Returns the dimensions of the render target.
    pub fn size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }

    /// Returns the surface format.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    /// Returns the sample count (always 1 for now).
    pub fn sample_count(&self) -> u32 {
        1
    }

    /// Returns the scale factor (always 1.0 for headless rendering).
    pub fn scale_factor(&self) -> f64 {
        1.0
    }

    /// Reads pixels from the offscreen framebuffer into a buffer.
    ///
    /// This captures the current rendered frame as RGB pixel data.
    /// Pixels are stored in RGB format (3 bytes per pixel), row by row from bottom to top.
    ///
    /// # Arguments
    /// * `out` - The output buffer. It will be resized to width × height × 3 bytes.
    /// * `x` - The x-coordinate of the region to read (always 0 for now)
    /// * `y` - The y-coordinate of the region to read (always 0 for now)
    /// * `width` - The width of the region to read
    /// * `height` - The height of the region to read
    pub fn read_pixels(&self, out: &mut Vec<u8>, x: usize, y: usize, width: usize, height: usize) {
        pixel_reading::read_texture_to_rgb(
            &self.offscreen_buffers.color_texture,
            out,
            x,
            y,
            width,
            height,
            self.surface_config.format,
            "drm_screenshot",
        )
    }

    /// Creates a readback texture for screenshots
    fn create_readback_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        texture_utils::create_readback_texture(
            device,
            width,
            height,
            format,
            "drm_readback_texture",
        )
    }
}

/// Wrapper for the surface texture to match wgpu::SurfaceTexture API.
pub struct DrmSurfaceTexture<'a> {
    pub texture: &'a wgpu::Texture,
}

impl Drop for DrmCanvas {
    fn drop(&mut self) {
        // Decrement window count and reset context if this is the last window
        if Context::decrement_window_count() {
            log::info!("Last DRM canvas dropped, resetting wgpu context");
            Context::reset();
        }
    }
}

#[cfg(feature = "drm")]
impl DrmCanvas {
    /// Query display resources and find a suitable display configuration.
    fn query_display_resources(card: &Card) -> Result<DisplayConfig, DrmCanvasError> {
        log::info!("Querying DRM display resources");

        // Get resource handles
        let resources = card.resource_handles().map_err(|e| {
            DrmCanvasError::DrmError(format!("Failed to get resource handles: {}", e))
        })?;

        // Find connected connector
        let connector_info = Self::find_connected_connector(card, &resources)?;
        log::info!(
            "Found connected connector: {:?} (id: {:?})",
            connector_info.interface(),
            connector_info.handle()
        );

        // Find available CRTC
        let crtc = Self::find_available_crtc(card, &connector_info, &resources)?;
        log::info!("Selected CRTC: {:?}", crtc);

        // Select best mode
        let mode = Self::select_best_mode(&connector_info)?;
        let (width, height) = mode.size();
        log::info!(
            "Selected mode: {}x{} @ {}Hz",
            width,
            height,
            mode.vrefresh()
        );

        Ok(DisplayConfig {
            connector: connector_info.handle(),
            crtc,
            mode,
            width: width as u32,
            height: height as u32,
        })
    }

    /// Find the first connected connector.
    fn find_connected_connector(
        card: &Card,
        resources: &ResourceHandles,
    ) -> Result<connector::Info, DrmCanvasError> {
        for &conn_handle in resources.connectors() {
            let conn_info = card.get_connector(conn_handle, false).map_err(|e| {
                DrmCanvasError::DrmError(format!("Failed to get connector info: {}", e))
            })?;

            if conn_info.state() == connector::State::Connected {
                return Ok(conn_info);
            }
        }

        Err(DrmCanvasError::NoConnectedDisplay)
    }

    /// Find an available CRTC for the given connector.
    fn find_available_crtc(
        _card: &Card,
        _connector_info: &connector::Info,
        resources: &ResourceHandles,
    ) -> Result<crtc::Handle, DrmCanvasError> {
        // For simplicity, just use the first available CRTC
        // A more sophisticated implementation would check encoder compatibility
        resources
            .crtcs()
            .first()
            .copied()
            .ok_or_else(|| DrmCanvasError::ModesetError("No CRTCs available".to_string()))
    }

    /// Select the best display mode (usually the preferred/native mode).
    fn select_best_mode(connector_info: &connector::Info) -> Result<Mode, DrmCanvasError> {
        let modes = connector_info.modes();

        if modes.is_empty() {
            return Err(DrmCanvasError::ModesetError(
                "No display modes available".to_string(),
            ));
        }

        // The first mode is typically the preferred/native resolution
        Ok(*modes.first().unwrap())
    }

    /// Choose compatible pixel formats for wgpu and DRM.
    fn choose_formats() -> FormatInfo {
        // Use XRGB8888 for maximum compatibility with displays
        // Note: wgpu uses BGRA8Unorm which we'll need to convert
        FormatInfo {
            wgpu_format: wgpu::TextureFormat::Bgra8Unorm,
            drm_format: DrmFourcc::Xrgb8888,
        }
    }

    pub fn copy_frame_to_readback(&self, frame: &DrmSurfaceTexture) {
        let ctxt = Context::get();
        let mut encoder = ctxt.create_command_encoder(Some("readback_copy_encoder"));

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.readback_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.surface_config.width,
                height: self.surface_config.height,
                depth_or_array_layers: 1,
            },
        );

        ctxt.submit(std::iter::once(encoder.finish()));
    }

    /// Read GPU texture data into a CPU buffer.
    /// This transfers rendered frame data from GPU to CPU memory.
    fn read_texture_to_buffer(
        texture: &wgpu::Texture,
        buffer: &mut Vec<u8>,
        width: u32,
        height: u32,
    ) -> Result<(), DrmCanvasError> {
        log::debug!("Reading GPU texture to CPU buffer");

        pixel_reading::read_texture_to_buffer(texture, buffer, width, height)
            .map_err(|e| DrmCanvasError::Init(e))?;

        log::debug!("GPU texture read complete");
        Ok(())
    }
}

// Trait implementations for DrmCanvas

impl RenderCanvas for DrmCanvas {
    type Frame<'a>
        = DrmSurfaceTexture<'a>
    where
        Self: 'a;
    type Error = DrmCanvasError;

    fn get_current_texture(&self) -> Result<Self::Frame<'_>, Self::Error> {
        Ok(DrmSurfaceTexture {
            texture: &self.offscreen_buffers.color_texture,
        })
    }

    fn present(&self, _frame: Self::Frame<'_>) -> Result<(), Self::Error> {
        match &self.mode {
            RenderMode::Offscreen => {
                // No-op for offscreen rendering
                Ok(())
            }
            #[cfg(feature = "drm")]
            RenderMode::Display(display) => {
                log::debug!("Present: starting frame presentation");

                let width = self.surface_config.width;
                let height = self.surface_config.height;

                // Step 1: Get a buffer from the pool (blocks if none available)
                let mut pixel_buffer = display.buffer_pool.try_get_buffer().unwrap_or_else(|| {
                    log::warn!("No buffer available, allocating new buffer");
                    vec![0u8; (width * height * 4) as usize]
                });

                // Step 2: Read GPU texture into the CPU buffer
                Self::read_texture_to_buffer(
                    &self.offscreen_buffers.color_texture,
                    &mut pixel_buffer,
                    width,
                    height,
                )?;
                log::debug!("Present: read GPU texture to CPU buffer");

                // Step 3: Send pixel data to display thread (non-blocking!)
                let command = DisplayCommand {
                    pixel_data: pixel_buffer,
                    width,
                    height,
                };

                if let Err(e) = display.display_thread.send_frame(command) {
                    log::error!("Failed to send frame to display thread: {}", e);
                    return Err(DrmCanvasError::PageFlipError(format!(
                        "Failed to send frame to display thread: {}",
                        e
                    )));
                }

                log::trace!("Frame queued for async display - main thread continues");
                log::debug!("Present: complete");
                Ok(())
            }
        }
    }

    fn depth_view(&self) -> &wgpu::TextureView {
        &self.offscreen_buffers.depth_view
    }

    fn msaa_view(&self) -> Option<&wgpu::TextureView> {
        None
    }

    fn sample_count(&self) -> u32 {
        1
    }

    fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    fn size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }
}

impl ScreenshotCanvas for DrmCanvas {
    type Frame<'a>
        = DrmSurfaceTexture<'a>
    where
        Self: 'a;

    fn copy_frame_to_readback(&self, frame: &Self::Frame<'_>) {
        let ctxt = Context::get();
        let mut encoder = ctxt.create_command_encoder(Some("readback_copy_encoder"));

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.readback_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.surface_config.width,
                height: self.surface_config.height,
                depth_or_array_layers: 1,
            },
        );

        ctxt.submit(std::iter::once(encoder.finish()));
    }

    fn read_pixels(&self, out: &mut Vec<u8>, x: usize, y: usize, width: usize, height: usize) {
        pixel_reading::read_texture_to_rgb(
            &self.offscreen_buffers.color_texture,
            out,
            x,
            y,
            width,
            height,
            self.surface_config.format,
            "drm_screenshot",
        )
    }
}

impl<'a> Drop for DrmSurfaceTexture<'a> {
    fn drop(&mut self) {
        // No-op: DRM doesn't need to present on drop like regular SurfaceTexture
    }
}
