//! Rendering functionality.

#![allow(clippy::await_holding_refcell_ref)]

use crate::camera::{Camera2d, Camera3d};
use crate::post_processing::PostProcessingEffect;
use crate::renderer::Renderer3d;
use crate::scene::{SceneNode2d, SceneNode3d};

use super::Window;

impl Window {
    /// Renders a 3D scene to the offscreen buffer.
    ///
    /// This is the main rendering method for DRM windows.
    /// It should be called once per frame in your render loop.
    ///
    /// # Arguments
    /// * `scene` - The 3D scene graph to render
    /// * `camera` - The camera used for viewing the scene
    ///
    /// # Returns
    /// Always returns `true` (no window close events in headless mode)
    ///
    /// # Example
    /// ```no_run
    /// # use kiss3d::prelude::*;
    /// # #[kiss3d::main]
    /// # async fn main() {
    /// # let mut window = Window::new("/dev/dri/card0", 1920, 1080).await.unwrap();
    /// # let mut camera = OrbitCamera3d::default();
    /// # let mut scene = SceneNode3d::empty();
    /// while window.render_3d(&mut scene, &mut camera).await {
    ///     // Per-frame updates here
    /// }
    /// # }
    /// ```
    pub async fn render_single_frame(
        &mut self,
        scene: Option<&mut SceneNode3d>,
        mut scene_2d: Option<&mut SceneNode2d>,
        camera: &mut dyn Camera3d,
        camera_2d: &mut dyn Camera2d,
        mut renderer: Option<&mut dyn Renderer3d>,
        mut post_processing: Option<&mut dyn PostProcessingEffect>,
    ) -> bool {
        use crate::context::Context;
        use crate::event::WindowEvent;
        use crate::resource::RenderContext2dEncoder;
        use crate::window::Canvas;

        let w = self.width();
        let h = self.height();

        // Create a canvas wrapper for camera compatibility
        let canvas_wrapper = super::DrmCanvasWrapper::new(&self.canvas);

        // SAFETY: Cameras need a &Canvas reference, but in headless mode most
        // don't actually use it. This transmute is safe because:
        // 1. The camera only reads from the canvas (no writes)
        // 2. The lifetime is constrained to this function scope
        // 3. DrmCanvasWrapper provides compatible methods for camera calls
        let canvas_ref: &Canvas = unsafe { std::mem::transmute(&canvas_wrapper) };

        // Update camera state
        camera.handle_event(canvas_ref, &WindowEvent::FramebufferSize(w, h));
        camera.update(canvas_ref);

        // Get the surface texture
        let frame = match self.canvas.get_current_texture() {
            Ok(frame) => frame,
            Err(e) => {
                eprintln!("Failed to acquire DRM surface texture: {:?}", e);
                return true; // Continue rendering in headless mode
            }
        };
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let ctxt = Context::get();
        let mut encoder = ctxt.create_command_encoder(Some("drm_frame_encoder"));

        // Resize post-process render target if needed
        let surface_format = self.canvas.surface_format();
        self.post_process_render_target.resize(w, h, surface_format);

        // Render directly to the frame (no post-processing for now)
        let color_view = &frame_view;
        let depth_view = self.canvas.depth_view();

        // Use shared rendering pipeline
        crate::window::rendering::render_frame_3d(
            &mut encoder,
            color_view,
            depth_view,
            surface_format,
            self.canvas.sample_count(),
            w,
            h,
            self.background,
            self.ambient_intensity,
            scene,
            camera,
            canvas_ref,
            &mut self.point_renderer,
            &mut self.polyline_renderer,
            &mut None, // No custom renderer for now
        );

        // Render text overlay
        {
            let mut context_2d_encoder = RenderContext2dEncoder {
                encoder: &mut encoder,
                color_view,
                surface_format,
                sample_count: self.canvas.sample_count(),
                viewport_width: w,
                viewport_height: h,
            };

            self.text_renderer
                .render(w as f32, h as f32, &mut context_2d_encoder);
        }

        // Submit commands
        ctxt.submit(std::iter::once(encoder.finish()));

        // Present the frame
        if let Err(e) = self.canvas.present() {
            log::error!("Failed to present frame: {}", e);
        }

        true // Always continue in headless mode
    }
}
