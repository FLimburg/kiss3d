//! DRM Window for headless 3D rendering without a window manager.

use super::drm_canvas::DrmCanvas;
use super::drm_events::DrmEventManager;
use crate::color::BLACK;
use crate::context::Context;
use crate::renderer::{PointRenderer2d, PointRenderer3d, PolylineRenderer2d, PolylineRenderer3d};
use crate::resource::{FramebufferManager, MaterialManager2d, MeshManager2d, RenderTarget};
use crate::text::TextRenderer;
#[cfg(feature = "egui")]
use crate::window::egui_integration::EguiContext;
#[cfg(feature = "recording")]
use crate::window::recording::RecordingState;
use crate::window::window_cache::WindowCache;
use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

/// A window for headless 3D rendering using DRM (Direct Rendering Manager).
///
/// Suitable for console-only systems such as Raspberry Pi. All rendering logic
/// (drawing, screenshots, recording, render loop) is shared with the windowed
/// `Window` through the common `impl` blocks in the sibling modules.
pub struct Window {
    // ── DRM-specific ──────────────────────────────────────────────────────
    pub(crate) canvas: DrmCanvas,
    pub(crate) event_manager: Rc<RefCell<DrmEventManager>>,

    // ── Shared fields — must mirror window::Window exactly ────────────────
    pub(crate) ambient_intensity: f32,
    pub(crate) background: crate::color::Color,
    pub(crate) polyline_renderer_2d: PolylineRenderer2d,
    pub(crate) point_renderer_2d: PointRenderer2d,
    pub(crate) point_renderer: PointRenderer3d,
    pub(crate) polyline_renderer: PolylineRenderer3d,
    pub(crate) text_renderer: TextRenderer,
    #[allow(dead_code)]
    pub(crate) framebuffer_manager: FramebufferManager,
    pub(crate) post_process_render_target: RenderTarget,
    pub(crate) should_close: bool,
    #[cfg(feature = "egui")]
    pub(crate) egui_context: EguiContext,
    #[cfg(feature = "recording")]
    pub(crate) recording: Option<RecordingState>,
}

impl Window {
    // ── Constructors ──────────────────────────────────────────────────────

    /// Opens a DRM window connected to the given device path, using the
    /// display's native resolution.
    pub async fn try_new(device_path: &str) -> Result<Self, Box<dyn Error>> {
        let canvas = DrmCanvas::new_with_display(device_path).await?;
        Self::new_from_canvas(canvas).await
    }

    /// Creates a DRM window in offscreen-only mode (no display output).
    ///
    /// Useful for server-side rendering, testing, or recording without a monitor.
    pub async fn new_offscreen(width: u32, height: u32) -> Result<Self, Box<dyn Error>> {
        log::info!("Creating DRM window (offscreen only): {}x{}", width, height);
        let canvas = DrmCanvas::new(width, height).await?;
        Self::new_from_canvas(canvas).await
    }

    /// Common initialisation shared by all DRM constructors.
    async fn new_from_canvas(canvas: DrmCanvas) -> Result<Self, Box<dyn Error>> {
        WindowCache::populate();

        let framebuffer_manager = FramebufferManager::new();
        let (width, height) = canvas.size();
        let post_process_render_target = framebuffer_manager.new_render_target(width, height, true);

        log::info!("DRM window initialised successfully");

        Ok(Self {
            canvas,
            event_manager: Rc::new(RefCell::new(DrmEventManager::new_headless())),
            ambient_intensity: 0.2,
            background: BLACK,
            polyline_renderer_2d: PolylineRenderer2d::new(),
            point_renderer_2d: PointRenderer2d::new(),
            point_renderer: PointRenderer3d::new(),
            polyline_renderer: PolylineRenderer3d::new(),
            text_renderer: TextRenderer::new(),
            framebuffer_manager,
            post_process_render_target,
            should_close: false,
            #[cfg(feature = "egui")]
            egui_context: EguiContext::new(),
            #[cfg(feature = "recording")]
            recording: None,
        })
    }

    /// Opens the first available DRM device. Mirrors the `Window::new(title)` signature
    /// of the windowed backend so call-sites are interchangeable.
    pub async fn new(_title: &str) -> Self {
        let device_paths = [
            "/dev/dri/card0",
            "/dev/dri/card1",
            "/dev/dri/card2",
            "/dev/dri/renderD128",
            "/dev/dri/renderD129",
        ];
        for dev in device_paths {
            match Self::try_new(dev).await {
                Ok(window) => {
                    log::debug!("Created render target for device {dev}");
                    return window;
                }
                Err(e) => {
                    log::trace!("Could not create render target for device {dev}: {e}");
                }
            }
        }
        log::error!("Could not create any render target!");
        panic!("Could not create any render target!");
    }

    // ── DRM-specific event source configuration ───────────────────────────

    /// Switches the event source to evdev input devices (keyboard, mouse, touchscreen).
    #[cfg(target_os = "linux")]
    pub fn enable_evdev_input(&mut self, devices: Vec<String>) -> Result<(), std::io::Error> {
        let manager = DrmEventManager::new_with_evdev(devices)?;
        self.event_manager = Rc::new(RefCell::new(manager));
        log::info!("Evdev input enabled");
        Ok(())
    }

    /// Switches the event source to a custom channel (GPIO buttons, network control, etc.)
    pub fn set_custom_event_source(
        &mut self,
        receiver: std::sync::mpsc::Receiver<crate::event::WindowEvent>,
    ) {
        let manager = DrmEventManager::new_with_custom(receiver);
        self.event_manager = Rc::new(RefCell::new(manager));
        log::info!("Custom event source enabled");
    }

    /// Returns an event manager wrapper that iterates events polled this frame.
    pub fn events(&self) -> super::drm_events::DrmEventManagerWrapper {
        super::drm_events::DrmEventManagerWrapper::new(self.event_manager.clone())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // Only clean up GPU resources when the last window is dropped.
        // This prevents TLS access order issues with wgpu internals that can cause
        // panics during thread cleanup.
        let is_last_window = Context::decrement_window_count();

        if is_last_window {
            // The order matters: clear caches first (which hold references to GPU resources),
            // then clear the Context (which holds the wgpu Device/Queue/Instance).

            // Clear 3D resource managers
            WindowCache::reset();

            // Clear 2D resource managers
            MeshManager2d::reset_global_manager();
            MaterialManager2d::reset_global_manager();

            // Finally, clear the wgpu context itself
            Context::reset();
        }
    }
}
