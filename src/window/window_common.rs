//! Accessor methods shared between the windowed (`window::Window`) and DRM
//! (`drm::Window`) backends.
//!
//! Both `Window` types carry identically-named fields for all of these
//! operations, so a single unconditional `impl Window` block covers both.
//! Neither backend needs to define these methods individually.

use glamx::UVec2;

use crate::color::Color;
use crate::event::{Action, Key, MouseButton};

use super::Window;

impl Window {
    /// Indicates whether this window should be closed.
    ///
    /// Returns `true` after [`close()`](Self::close) has been called, or after
    /// an `Escape` key or window-close event has been received.
    #[inline]
    pub fn should_close(&self) -> bool {
        self.should_close
    }

    /// Closes the window.
    ///
    /// After calling this method, [`render()`](Self::render) will return `false`
    /// on the next frame, allowing the render loop to exit gracefully.
    #[inline]
    pub fn close(&mut self) {
        self.should_close = true;
    }

    /// Returns the width of the render target in pixels.
    #[inline]
    pub fn width(&self) -> u32 {
        self.canvas.size().0
    }

    /// Returns the height of the render target in pixels.
    #[inline]
    pub fn height(&self) -> u32 {
        self.canvas.size().1
    }

    /// Returns the dimensions of the render target.
    #[inline]
    pub fn size(&self) -> UVec2 {
        let (w, h) = self.canvas.size();
        UVec2::new(w, h)
    }

    /// Sets the background clear colour.
    ///
    /// # Arguments
    /// * `color` - The background colour to use
    ///
    /// # Example
    /// ```no_run
    /// # use kiss3d::window::Window;
    /// # #[kiss3d::main]
    /// # async fn main() {
    /// use kiss3d::color::DARK_BLUE;
    /// let mut window = Window::new("Example").await;
    /// window.set_background_color(DARK_BLUE);
    /// # }
    /// ```
    #[inline]
    pub fn set_background_color(&mut self, color: Color) {
        self.background = color;
    }

    /// Sets the ambient light intensity for the scene.
    ///
    /// # Arguments
    /// * `ambient` - The ambient light intensity (typically 0.0 to 1.0)
    ///
    /// # Example
    /// ```no_run
    /// # use kiss3d::window::Window;
    /// # #[kiss3d::main]
    /// # async fn main() {
    /// # let mut window = Window::new("Example").await;
    /// // Set global ambient lighting intensity
    /// window.set_ambient(0.3);
    /// # }
    /// ```
    ///
    /// Note: Individual lights should be added to the scene tree using
    /// `SceneNode3d::add_point_light()`, `add_directional_light()`, or `add_spot_light()`.
    #[inline]
    pub fn set_ambient(&mut self, ambient: f32) {
        self.ambient_intensity = ambient;
    }

    /// Returns the current ambient light intensity.
    #[inline]
    pub fn ambient(&self) -> f32 {
        self.ambient_intensity
    }

    /// Returns the DPI scale factor of the display.
    ///
    /// This is the ratio between physical pixels and logical pixels.
    /// On high-DPI displays (like Retina displays) this will be greater than 1.0.
    /// On the DRM/headless backend this always returns 1.0.
    ///
    /// # Returns
    /// The scale factor (e.g., 1.0 for standard displays, 2.0 for Retina displays)
    #[inline]
    pub fn scale_factor(&self) -> f64 {
        self.canvas.scale_factor()
    }

    /// Returns the last known position of the mouse cursor, or `None` if unknown.
    ///
    /// The position is automatically updated when the mouse moves over the window.
    /// Coordinates are in pixels, with (0, 0) at the top-left corner.
    /// On the DRM/headless backend this always returns `None`.
    ///
    /// # Returns
    /// `Some((x, y))` with the cursor position, or `None` if the cursor position is unknown
    #[inline]
    pub fn cursor_pos(&self) -> Option<(f64, f64)> {
        self.canvas.cursor_pos()
    }

    /// Returns the current state of a keyboard key.
    ///
    /// # Arguments
    /// * `key` - The key to check
    ///
    /// # Returns
    /// The current `Action` state (e.g., `Action::Press`, `Action::Release`)
    ///
    /// On the DRM/headless backend this always returns `Action::Release`.
    #[inline]
    pub fn get_key(&self, key: Key) -> Action {
        self.canvas.get_key(key)
    }

    /// Returns the current state of a mouse button.
    ///
    /// # Arguments
    /// * `button` - The mouse button to check
    ///
    /// # Returns
    /// The current `Action` state (e.g., `Action::Press`, `Action::Release`)
    ///
    /// On the DRM/headless backend this always returns `Action::Release`.
    #[inline]
    pub fn get_mouse_button(&self, button: MouseButton) -> Action {
        self.canvas.get_mouse_button(button)
    }
}
