//! DRM Canvas for headless rendering without a window manager.

use crate::context::Context;
use crate::resource::OffscreenBuffers;
use crate::window::canvas::CanvasInputState;
use std::cell::RefCell;
use std::error::Error;
use std::fmt;

use super::card::Card;
use super::display_thread::{BufferPool, DisplayCommand, DisplayThread, DisplayThreadConfig};
use drm::buffer::DrmFourcc;
use drm::control::{connector, crtc, Device as ControlDevice, Mode, ResourceHandles};

/// Error type for DRM canvas operations.
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
struct DisplayConfig {
    connector: connector::Handle,
    crtc: crtc::Handle,
    mode: Mode,
    width: u32,
    height: u32,
}

/// Format compatibility information
struct FormatInfo {
    wgpu_format: wgpu::TextureFormat,
    drm_format: DrmFourcc,
}

/// DRM display state for actual screen output
struct DrmDisplayState {
    /// Async display thread for non-blocking presentation
    display_thread: DisplayThread,
    /// Buffer pool for recycling pixel buffers
    buffer_pool: BufferPool,
    /// Pipelined readback manager for non-blocking GPU→CPU transfers
    /// Wrapped in RefCell for interior mutability (accessed only from render thread)
    readback_pipeline: RefCell<ReadbackPipeline>,
}

/// Manages pipelined GPU→CPU readback with multiple staging buffers
struct ReadbackPipeline {
    /// Rotating staging buffers (typically 2-3 for double/triple buffering)
    staging_buffers: Vec<StagingBuffer>,
    /// Index of the next staging buffer to use
    next_buffer_idx: usize,
    /// Frame dimensions
    width: u32,
    height: u32,
    /// Bytes per row (padded to alignment)
    padded_bytes_per_row: usize,
    /// Bytes per row (unpadded, actual pixel data)
    stride: usize,
}

/// A single staging buffer with its readback state
struct StagingBuffer {
    /// wgpu buffer for GPU→CPU transfer
    buffer: wgpu::Buffer,
    /// Current state of this buffer
    state: ReadbackState,
}

/// State of a staging buffer in the readback pipeline
enum ReadbackState {
    /// Buffer is available for new readback
    Available,
    /// GPU copy has been submitted, waiting for completion
    Pending {
        /// Channel to receive map completion notification
        receiver: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    },
    /// Buffer is mapped and ready to read
    Ready,
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

        // Step 8: Create pipelined readback for non-blocking GPU→CPU transfers
        log::info!("Creating pipelined readback system...");
        let readback_pipeline = ReadbackPipeline::new(
            display_config.width,
            display_config.height,
            3, // Triple buffering: can have 2 in flight + 1 being read
        );

        // Step 9: Create display state
        let display_state = DrmDisplayState {
            display_thread,
            buffer_pool,
            readback_pipeline: RefCell::new(readback_pipeline),
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
    pub fn present(&self, _frame: DrmSurfaceTexture) {
        match &self.mode {
            RenderMode::Offscreen => {
                // No-op for offscreen rendering
            }
            RenderMode::Display(display) => {
                log::debug!("Present: starting pipelined frame presentation");

                // Step 1: Check if any previous frame readback has completed
                if let Some(pixel_data) = display.readback_pipeline.borrow_mut().poll_ready_frame()
                {
                    // Step 2: Send completed frame to display thread
                    let command = DisplayCommand {
                        pixel_data,
                        width: self.surface_config.width,
                        height: self.surface_config.height,
                    };

                    // Step 3: Send to display thread (may block if channel is full)
                    if let Err(e) = display.display_thread.send_frame(command) {
                        log::error!("Failed to send frame to display thread: {}", e);
                    } else {
                        log::trace!("Frame sent to display thread");
                    }
                } else {
                    log::trace!("No ready frame yet, continuing");
                }

                // Step 4: Start readback for the current frame (non-blocking)
                // This initiates GPU→CPU copy that will complete asynchronously
                if display
                    .readback_pipeline
                    .borrow_mut()
                    .start_readback(&self.offscreen_buffers.color_texture)
                {
                    log::trace!("Readback started for current frame");
                } else {
                    log::trace!("Readback skipped: all staging buffers busy");
                }

                log::debug!("Present: complete (non-blocking)");
            }
        }
    }

    /// Returns the dimensions of the render target.
    pub fn size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }

    /// Returns a lightweight headless input state view (no keyboard/mouse in DRM mode).
    ///
    /// The returned value is `'static` — it borrows from fully-released static
    /// arrays, so no allocation takes place.
    pub fn input_state(&self) -> CanvasInputState<'static> {
        CanvasInputState::headless(self.size())
    }

    /// Returns the surface format.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_config.format
    }

    /// Returns the sample count (always 1 for now).
    pub fn sample_count(&self) -> u32 {
        1
    }

    /// Returns the DPI scale factor (always 1.0 for DRM/headless).
    pub fn scale_factor(&self) -> f64 {
        1.0
    }

    /// Returns the cursor position (always None — no pointer device in headless mode).
    pub fn cursor_pos(&self) -> Option<(f64, f64)> {
        None
    }

    /// Returns the state of a keyboard key (always Release — no keyboard in headless mode).
    pub fn get_key(&self, _key: crate::event::Key) -> crate::event::Action {
        crate::event::Action::Release
    }

    /// Returns the state of a mouse button (always Release — no pointer device in headless mode).
    pub fn get_mouse_button(&self, _button: crate::event::MouseButton) -> crate::event::Action {
        crate::event::Action::Release
    }

    /// No-op poll — DRM canvas receives events through DrmEventManager, not here.
    pub fn poll_events(&mut self) {}

    /// Copies the rendered frame texture to the readback texture.
    ///
    /// For DRM canvas the "frame" is already the offscreen color texture, so this
    /// is a texture-to-texture copy into the dedicated readback buffer.
    pub fn copy_frame_to_readback(&self, _frame: &DrmSurfaceTexture<'_>) {
        let ctxt = crate::context::Context::get();
        let mut encoder = ctxt.create_command_encoder(Some("drm_readback_copy_encoder"));

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.offscreen_buffers.color_texture,
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
        let ctxt = Context::get();

        // Calculate buffer size with alignment
        // wgpu requires rows to be aligned to 256 bytes
        let bytes_per_pixel = 4; // RGBA
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let buffer_size = padded_bytes_per_row * height;

        // Create staging buffer
        let staging_buffer = ctxt.create_buffer(&wgpu::BufferDescriptor {
            label: Some("drm_screenshot_staging_buffer"),
            size: buffer_size as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy from offscreen texture to staging buffer
        let mut encoder = ctxt.create_command_encoder(Some("drm_screenshot_copy_encoder"));

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.offscreen_buffers.color_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: x as u32,
                    y: y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row as u32),
                    rows_per_image: Some(height as u32),
                },
            },
            wgpu::Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
        );

        ctxt.submit(std::iter::once(encoder.finish()));

        // Map the buffer and read the data
        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });

        // Wait for the GPU to finish
        let _ = ctxt.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();

        // Read the data
        let data = buffer_slice.get_mapped_range();

        // Convert from RGBA to RGB and handle row padding
        let rgb_size = width * height * 3;
        out.clear();
        out.reserve(rgb_size);

        // Read rows in reverse order for bottom-left origin compatibility
        for row in (0..height).rev() {
            let row_start = row * padded_bytes_per_row;
            for col in 0..width {
                let pixel_start = row_start + col * bytes_per_pixel;
                // RGBA -> RGB
                out.push(data[pixel_start]); // R
                out.push(data[pixel_start + 1]); // G
                out.push(data[pixel_start + 2]); // B
            }
        }

        drop(data);
        staging_buffer.unmap();
    }

    /// Creates a readback texture for screenshots
    fn create_readback_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("drm_readback_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }
}

/// Wrapper for the surface texture to match wgpu::SurfaceTexture API.
pub struct DrmSurfaceTexture<'a> {
    pub texture: &'a wgpu::Texture,
}

impl Drop for DrmCanvas {
    fn drop(&mut self) {
        // DRM/KMS resources (dumb buffers, framebuffers) are cleaned up by
        // DisplayThread's own Drop impl. Context lifecycle (decrement_window_count /
        // reset) is managed exclusively by Window::drop to avoid double-decrement.
        log::debug!("DrmCanvas dropped");
    }
}

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

    /// Read GPU texture data into a CPU buffer.
    /// This transfers rendered frame data from GPU to CPU memory.
    fn read_texture_to_buffer(
        texture: &wgpu::Texture,
        buffer: &mut Vec<u8>,
        width: u32,
        height: u32,
    ) -> Result<(), DrmCanvasError> {
        log::debug!("Reading GPU texture to CPU buffer");

        let ctxt = Context::get();

        // Calculate buffer layout with proper alignment
        let bytes_per_pixel = 4; // RGBA/XRGB
        let unpadded_bytes_per_row = width as usize * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let buffer_size = padded_bytes_per_row * height as usize;

        // Create staging buffer for GPU -> CPU transfer
        let staging_buffer = ctxt.create_buffer(&wgpu::BufferDescriptor {
            label: Some("drm_frame_read_staging"),
            size: buffer_size as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy texture to staging buffer
        let mut encoder = ctxt.create_command_encoder(Some("drm_frame_read_encoder"));
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row as u32),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        ctxt.submit(std::iter::once(encoder.finish()));

        // Map staging buffer and read data
        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });

        // Wait for GPU to finish
        let _ = ctxt.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv()
            .map_err(|e| {
                DrmCanvasError::Init(format!("Failed to receive buffer map result: {}", e))
            })?
            .map_err(|e| DrmCanvasError::Init(format!("Failed to map staging buffer: {}", e)))?;

        let mapped_data = buffer_slice.get_mapped_range();

        // Resize output buffer if needed
        let stride = (width * 4) as usize; // 4 bytes per pixel
        let output_size = stride * height as usize;
        buffer.resize(output_size, 0);

        // Copy data, handling potential stride differences
        if padded_bytes_per_row == stride {
            // Fast path: strides match, copy entire buffer at once
            buffer[..output_size].copy_from_slice(&mapped_data[..output_size]);
        } else {
            // Slow path: different strides, copy row by row
            for y in 0..height as usize {
                let src_row_start = y * padded_bytes_per_row;
                let dst_row_start = y * stride;
                buffer[dst_row_start..dst_row_start + stride]
                    .copy_from_slice(&mapped_data[src_row_start..src_row_start + stride]);
            }
        }

        drop(mapped_data);
        staging_buffer.unmap();

        log::debug!("GPU texture read complete");
        Ok(())
    }
}

impl ReadbackPipeline {
    /// Create a new pipelined readback system with multiple staging buffers
    fn new(width: u32, height: u32, num_buffers: usize) -> Self {
        let ctxt = Context::get();

        // Calculate buffer layout with proper alignment
        let bytes_per_pixel = 4; // RGBA/XRGB
        let unpadded_bytes_per_row = width as usize * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;
        let buffer_size = padded_bytes_per_row * height as usize;

        log::info!(
            "Readback pipeline: {}x{}, {} buffers, {} bytes per buffer",
            width,
            height,
            num_buffers,
            buffer_size
        );

        // Create staging buffers
        let staging_buffers = (0..num_buffers)
            .map(|i| {
                let buffer = ctxt.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("readback_staging_{}", i)),
                    size: buffer_size as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                StagingBuffer {
                    buffer,
                    state: ReadbackState::Available,
                }
            })
            .collect();

        Self {
            staging_buffers,
            next_buffer_idx: 0,
            width,
            height,
            padded_bytes_per_row,
            stride: unpadded_bytes_per_row,
        }
    }

    /// Start a GPU→CPU copy for the given texture (non-blocking)
    /// Returns true if a copy was started, false if no buffers available
    fn start_readback(&mut self, texture: &wgpu::Texture) -> bool {
        let ctxt = Context::get();

        // Find an available staging buffer
        let buffer_idx = match self.find_available_buffer() {
            Some(idx) => idx,
            None => {
                log::trace!("Readback skipped: all staging buffers busy");
                return false;
            }
        };

        let staging = &mut self.staging_buffers[buffer_idx];

        // Issue GPU copy command
        let mut encoder = ctxt.create_command_encoder(Some("readback_copy"));
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row as u32),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        ctxt.submit(std::iter::once(encoder.finish()));

        // Start async map operation
        let buffer_slice = staging.buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).ok();
        });

        // Update state to Pending
        staging.state = ReadbackState::Pending { receiver: rx };

        log::trace!("Readback started on staging buffer {}", buffer_idx);
        true
    }

    /// Poll for completed readbacks and extract pixel data
    /// Returns Some(pixels) if a frame is ready, None otherwise
    fn poll_ready_frame(&mut self) -> Option<Vec<u8>> {
        let ctxt = Context::get();

        // Non-blocking poll to advance GPU work
        let _ = ctxt.device.poll(wgpu::PollType::Poll);

        // Check all buffers for ready frames
        for staging in &mut self.staging_buffers {
            if let ReadbackState::Pending { receiver } = &staging.state {
                // Try to receive map result (non-blocking)
                match receiver.try_recv() {
                    Ok(Ok(())) => {
                        // Buffer is now mapped and ready to read
                        staging.state = ReadbackState::Ready;
                    }
                    Ok(Err(e)) => {
                        log::error!("Buffer map failed: {:?}", e);
                        staging.state = ReadbackState::Available;
                        continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        // Still pending
                        continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        log::error!("Map callback channel disconnected");
                        staging.state = ReadbackState::Available;
                        continue;
                    }
                }
            }

            // If we reach here and state is Ready, extract the data
            if matches!(staging.state, ReadbackState::Ready) {
                let buffer_slice = staging.buffer.slice(..);
                let mapped_data = buffer_slice.get_mapped_range();

                // Allocate output buffer
                let output_size = self.stride * self.height as usize;
                let mut pixels = vec![0u8; output_size];

                // Copy data, handling potential stride differences
                if self.padded_bytes_per_row == self.stride {
                    // Fast path: strides match
                    pixels[..output_size].copy_from_slice(&mapped_data[..output_size]);
                } else {
                    // Slow path: copy row by row
                    for y in 0..self.height as usize {
                        let src_row_start = y * self.padded_bytes_per_row;
                        let dst_row_start = y * self.stride;
                        pixels[dst_row_start..dst_row_start + self.stride].copy_from_slice(
                            &mapped_data[src_row_start..src_row_start + self.stride],
                        );
                    }
                }

                drop(mapped_data);
                staging.buffer.unmap();
                staging.state = ReadbackState::Available;

                log::trace!("Frame extracted from staging buffer");
                return Some(pixels);
            }
        }

        None
    }

    /// Find an available staging buffer
    fn find_available_buffer(&mut self) -> Option<usize> {
        // Try to find an Available buffer starting from next_buffer_idx
        for i in 0..self.staging_buffers.len() {
            let idx = (self.next_buffer_idx + i) % self.staging_buffers.len();
            if matches!(self.staging_buffers[idx].state, ReadbackState::Available) {
                self.next_buffer_idx = (idx + 1) % self.staging_buffers.len();
                return Some(idx);
            }
        }
        None
    }
}

impl<'a> Drop for DrmSurfaceTexture<'a> {
    fn drop(&mut self) {
        // No-op: DRM doesn't need to present on drop like regular SurfaceTexture
    }
}
