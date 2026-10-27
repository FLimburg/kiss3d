//! Event handling functionality.

use crate::camera::Camera2d;
use crate::camera::Camera3d;
use crate::event::{Action, WindowEvent};
#[cfg(not(feature = "drm"))]
use crate::event::EventManager;

use super::Window;

#[cfg(feature = "rt_switcher")]
use crate::prelude::{RayTracer, RayTracerPreset};

impl Window {
    /// Returns an event manager for accessing window events.
    ///
    /// # Example
    /// ```no_run
    /// # use kiss3d::prelude::*;
    /// # #[kiss3d::main]
    /// # async fn main() {
    /// # let mut window = Window::new("Example").await;
    /// # let mut camera = OrbitCamera3d::default();
    /// # let mut scene = SceneNode3d::empty();
    /// # while window.render_3d(&mut scene, &mut camera).await {
    /// for event in window.events().iter() {
    ///     match event.value {
    ///         WindowEvent::Key(Key::Escape, Action::Release, _) => {
    ///             println!("Escape pressed!");
    ///         }
    ///         _ => {}
    ///     }
    /// }
    /// # }
    /// # }
    /// ```
    #[cfg(not(feature = "drm"))]
    pub fn events(&self) -> EventManager {
        EventManager::new(self.events.clone(), self.unhandled_events.clone())
    }

    /// Poll and dispatch all pending events to cameras.
    ///
    /// Called once per frame by `render` before drawing.
    #[inline]
    pub(crate) fn handle_events(
        &mut self,
        camera: &mut dyn Camera3d,
        camera_2d: &mut dyn Camera2d,
    ) {
        #[cfg(not(feature = "drm"))]
        {
            let unhandled_events = self.unhandled_events.clone(); // TODO: could we avoid the clone?
            let events = self.events.clone(); // TODO: could we avoid the clone?

            for event in unhandled_events.borrow().iter() {
                self.handle_event(camera, camera_2d, event);
            }

            for event in events.try_iter() {
                self.handle_event(camera, camera_2d, &event);
            }

            unhandled_events.borrow_mut().clear();
            self.canvas.poll_events();
        }

        #[cfg(feature = "drm")]
        {
            self.event_manager.borrow_mut().poll_events();
            let events: Vec<_> = self.event_manager.borrow_mut().drain_events().collect();
            for event in events {
                self.handle_event(camera, camera_2d, &event);
            }
        }
    }

    /// Dispatch a single event to cameras and handle built-in actions (close, egui, etc.).
    ///
    /// Shared between windowed and DRM backends.
    pub(crate) fn handle_event(
        &mut self,
        camera: &mut dyn Camera3d,
        camera_2d: &mut dyn Camera2d,
        event: &WindowEvent,
    ) {
        if let Some(binding_key) = self.close_key {
            if let WindowEvent::Key(key, Action::Release, modifiers) = event {
                if binding_key == *key
                    && (Some(*modifiers) == self.close_modifiers || self.close_modifiers.is_none())
                {
                    self.close()
                }
            }
        }

        #[cfg(feature = "rt_switcher")]
        match event {
            WindowEvent::Key(Key::F4, Action::Release, _) => {
                self.raytracer.1 = false;
            }
            WindowEvent::Key(Key::F5, Action::Release, _) => {
                self.raytracer.0 = Some(RayTracer::preset(RayTracerPreset::Low));
            }
            WindowEvent::Key(Key::F6, Action::Release, _) => {
                self.raytracer.0 = Some(RayTracer::preset(RayTracerPreset::Medium));
            }
            WindowEvent::Key(Key::F7, Action::Release, _) => {
                self.raytracer.0 = Some(RayTracer::preset(RayTracerPreset::High));
            }
            WindowEvent::Key(Key::F8, Action::Release, _) => {
                self.raytracer.0 = Some(RayTracer::preset(RayTracerPreset::Ultra));
            }
            _ => {}
        }

        if *event == WindowEvent::Close {
            self.close();
        }

        // Feed events to egui and check if it wants to capture input
        #[cfg(all(feature = "egui", not(feature = "drm")))]
        {
            self.feed_egui_event(event);

            if event.is_keyboard_event() && self.is_egui_capturing_keyboard() {
                return;
            }

            if event.is_mouse_event() && self.is_egui_capturing_mouse() {
                return;
            }
        }

        let input = self.canvas.input_state();
        camera.handle_event(&input, event);
        camera_2d.handle_event(&input, event);
    }
}
