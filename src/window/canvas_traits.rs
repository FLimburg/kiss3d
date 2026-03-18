//! Trait hierarchy for canvas abstractions.
//!
//! This module defines the core traits that different canvas implementations
//! (windowed, headless, DRM, etc.) can implement to provide a unified interface.

use crate::event::{Action, Key, MouseButton};
use image::{GenericImage, Pixel};

/// Core rendering capabilities shared by all canvas types.
///
/// This trait defines the minimal interface needed for rendering:
/// getting a render target, presenting frames, and querying render configuration.
pub trait RenderCanvas {
    /// The frame/texture type returned by this canvas.
    ///
    /// This uses a GAT (Generic Associated Type) to allow borrowed frames.
    type Frame<'a>
    where
        Self: 'a;

    /// Error type for rendering operations.
    type Error: std::error::Error;

    /// Gets the current render target/texture.
    ///
    /// For windowed canvases, this acquires a surface texture.
    /// For headless canvases, this returns a reference to an offscreen texture.
    fn get_current_texture(&self) -> Result<Self::Frame<'_>, Self::Error>;

    /// Presents the rendered frame.
    ///
    /// For windowed canvases, this displays the frame on screen.
    /// For headless canvases, this may send to a display thread or be a no-op.
    fn present(&self, frame: Self::Frame<'_>) -> Result<(), Self::Error>;

    /// Gets the depth texture view for rendering.
    fn depth_view(&self) -> &wgpu::TextureView;

    /// Gets the MSAA texture view if multisampling is enabled.
    ///
    /// Returns `None` if MSAA is disabled (sample count = 1).
    fn msaa_view(&self) -> Option<&wgpu::TextureView>;

    /// Gets the sample count for multisampling.
    ///
    /// Returns 1 if MSAA is disabled.
    fn sample_count(&self) -> u32;

    /// Gets the surface/texture format used for rendering.
    fn surface_format(&self) -> wgpu::TextureFormat;

    /// Gets the dimensions of the render target (width, height).
    fn size(&self) -> (u32, u32);
}

/// Screenshot and pixel reading capabilities.
///
/// This trait provides functionality for reading back rendered frames from the GPU.
/// It's separate from `RenderCanvas` because not all rendering contexts may support
/// or need pixel readback.
pub trait ScreenshotCanvas {
    /// The frame/texture type used by this canvas.
    ///
    /// This uses a GAT (Generic Associated Type) to allow borrowed frames.
    type Frame<'a>
    where
        Self: 'a;

    /// Copies the current frame to an internal readback texture.
    ///
    /// This is a prerequisite for `read_pixels()` and should be called after
    /// rendering is complete but before the frame is presented (if you want
    /// to read back the presented frame).
    fn copy_frame_to_readback(&self, frame: &Self::Frame<'_>);

    /// Reads pixels from the readback texture into a buffer.
    ///
    /// Returns RGB data (3 bytes per pixel) in bottom-to-top row order
    /// (OpenGL convention). The buffer will be resized to width × height × 3.
    ///
    /// # Arguments
    /// * `out` - Output buffer (will be cleared and resized)
    /// * `x` - Left coordinate of region to read
    /// * `y` - Bottom coordinate of region to read
    /// * `width` - Width of region to read
    /// * `height` - Height of region to read
    ///
    /// # Note
    /// You must call `copy_frame_to_readback()` first, otherwise you'll read
    /// stale or uninitialized data.
    fn read_pixels(&self, out: &mut Vec<u8>, x: usize, y: usize, width: usize, height: usize);
}

/// Window-specific capabilities (input, cursor, window management).
///
/// This trait is only implemented by windowed canvases that have an actual
/// OS window. Headless/offscreen canvases do not implement this trait.
pub trait WindowCanvas {
    /// Polls and processes window events.
    ///
    /// This should be called once per frame to pump the event loop and
    /// update internal state (cursor position, key states, etc.).
    fn poll_events(&mut self);

    /// Gets the current cursor position, if known.
    ///
    /// Returns `None` if the cursor position hasn't been reported yet
    /// or if the cursor is outside the window.
    fn cursor_pos(&self) -> Option<(f64, f64)>;

    /// Gets the window's scale factor (DPI scaling).
    ///
    /// For example, on a Retina display this might return 2.0.
    fn scale_factor(&self) -> f64;

    /// Sets the window title.
    fn set_title(&mut self, title: &str);

    /// Sets the window icon.
    ///
    /// The icon should be an RGBA image. Common sizes are 16×16, 32×32, 48×48.
    fn set_icon(&mut self, icon: impl GenericImage<Pixel = impl Pixel<Subpixel = u8>>);

    /// Sets whether the cursor is confined to the window.
    ///
    /// When `true`, the cursor cannot leave the window bounds.
    fn set_cursor_grab(&self, grab: bool);

    /// Sets the cursor position within the window.
    ///
    /// Coordinates are in physical pixels from the top-left corner.
    fn set_cursor_position(&self, x: f64, y: f64);

    /// Sets whether the cursor is visible.
    ///
    /// When `true`, the cursor is hidden when over the window.
    fn hide_cursor(&self, hide: bool);

    /// Hides the window (makes it invisible).
    fn hide(&mut self);

    /// Shows the window (makes it visible).
    fn show(&mut self);

    /// Gets the current state of a mouse button.
    ///
    /// Returns `Action::Press` if the button is currently pressed,
    /// `Action::Release` otherwise.
    fn get_mouse_button(&self, button: MouseButton) -> Action;

    /// Gets the current state of a keyboard key.
    ///
    /// Returns `Action::Press` if the key is currently pressed,
    /// `Action::Release` otherwise.
    fn get_key(&self, key: Key) -> Action;
}

/// Canvas interface required by camera implementations.
///
/// This trait defines the minimal interface that cameras need to interact
/// with the canvas. It's a subset of `WindowCanvas` that can also be implemented
/// by headless canvases, allowing cameras to work in both windowed and headless
/// contexts without requiring unsafe transmute operations.
///
/// Cameras primarily need to:
/// - Query input state (mouse buttons, keys) for interactive controls
/// - Get the scale factor for DPI-aware rendering
///
/// Note: Any type implementing `WindowCanvas` automatically implements `CameraCanvas`.
pub trait CameraCanvas {
    /// Gets the window's scale factor (DPI scaling).
    ///
    /// For example, on a Retina display this might return 2.0.
    /// For headless rendering, this returns 1.0.
    fn scale_factor(&self) -> f64;

    /// Gets the current state of a mouse button.
    ///
    /// Returns `Action::Press` if the button is currently pressed,
    /// `Action::Release` otherwise. In headless mode, always returns `Release`.
    fn get_mouse_button(&self, button: MouseButton) -> Action;

    /// Gets the current state of a keyboard key.
    ///
    /// Returns `Action::Press` if the key is currently pressed,
    /// `Action::Release` otherwise. In headless mode, always returns `Release`.
    fn get_key(&self, key: Key) -> Action;
}

/// Blanket implementation: Any WindowCanvas is also a CameraCanvas.
///
/// This makes sense because WindowCanvas has all the methods that cameras need
/// (scale_factor, get_mouse_button, get_key). This allows windowed canvases to
/// automatically work with camera code without explicit CameraCanvas implementation.
impl<T: WindowCanvas> CameraCanvas for T {
    fn scale_factor(&self) -> f64 {
        WindowCanvas::scale_factor(self)
    }

    fn get_mouse_button(&self, button: MouseButton) -> Action {
        WindowCanvas::get_mouse_button(self, button)
    }

    fn get_key(&self, key: Key) -> Action {
        WindowCanvas::get_key(self, key)
    }
}
