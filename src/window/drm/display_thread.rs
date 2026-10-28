//! Display thread: shows frames on the display without blocking the render thread.
//!
//! The render thread hands over RGB frames; this thread converts them to the
//! display's XRGB8888 layout, copies them into a dumb buffer and sets the CRTC.
//! Ported from the former DRM backend, which used the same double-buffered,
//! channel-based design.

use super::card::Card;
use drm::buffer::Buffer;
use drm::control::dumbbuffer::DumbBuffer;
use drm::control::{connector, crtc, framebuffer, Device as ControlDevice, Mode};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

/// A frame to display.
pub struct DisplayCommand {
    /// Tightly packed RGB pixels (3 bytes each), rows from top to bottom.
    pub rgb: Vec<u8>,
    /// Width of the frame in pixels.
    pub width: u32,
    /// Height of the frame in pixels.
    pub height: u32,
}

/// Where and how the display thread shows frames.
pub struct DisplayThreadConfig {
    pub connector: connector::Handle,
    pub crtc: crtc::Handle,
    pub mode: Mode,
}

/// Owns the worker thread that presents frames.
pub struct DisplayThread {
    /// Bounded channel (capacity 1): the render thread can queue one frame
    /// while the worker presents another, and blocks beyond that.
    sender: Option<SyncSender<DisplayCommand>>,
    thread_handle: Option<JoinHandle<()>>,
}

impl DisplayThread {
    /// Starts the display thread. `width` and `height` are the display size in pixels.
    pub fn new(card: Card, config: DisplayThreadConfig, width: u32, height: u32) -> Self {
        let (sender, receiver) = sync_channel(1);

        let thread_handle = thread::spawn(move || {
            Self::display_worker(card, config, receiver, width, height);
        });

        log::info!("Display thread started with bounded channel (capacity 1)");

        DisplayThread {
            sender: Some(sender),
            thread_handle: Some(thread_handle),
        }
    }

    /// Queues a frame for display.
    ///
    /// Blocks while the worker is still presenting the previous frame and one more
    /// is already queued, which gives natural backpressure.
    pub fn send_frame(&self, command: DisplayCommand) -> Result<(), String> {
        match &self.sender {
            Some(sender) => sender
                .send(command)
                .map_err(|e| format!("Failed to send display command: {}", e)),
            None => Err("Display thread sender has been closed".to_string()),
        }
    }

    fn display_worker(
        card: Card,
        config: DisplayThreadConfig,
        receiver: Receiver<DisplayCommand>,
        width: u32,
        height: u32,
    ) {
        log::debug!("Display worker thread started");

        // Double-buffered dumb buffers, alternated every frame.
        let mut dumb_buffer_front = Self::create_dumb_buffer(&card, width, height);
        let mut dumb_buffer_back = Self::create_dumb_buffer(&card, width, height);
        let fb_front = Self::create_framebuffer(&card, &dumb_buffer_front);
        let fb_back = Self::create_framebuffer(&card, &dumb_buffer_back);

        let mut use_front = true;

        // One row of XRGB pixels, converted in cached memory before the copy: the
        // mapped dumb buffer is typically uncached memory, slow for scattered writes.
        let mut row = vec![0u8; width as usize * 4];

        while let Ok(command) = receiver.recv() {
            let (dumb_buffer, fb) = if use_front {
                (&mut dumb_buffer_front, fb_front)
            } else {
                (&mut dumb_buffer_back, fb_back)
            };

            if let Err(e) = Self::copy_to_dumb_buffer(&card, dumb_buffer, &command, &mut row) {
                log::error!("Display worker: failed to copy to dumb buffer: {}", e);
                continue;
            }

            if let Err(e) = card.set_crtc(
                config.crtc,
                Some(fb),
                (0, 0),
                &[config.connector],
                Some(config.mode),
            ) {
                log::error!("Display worker: set_crtc failed: {}", e);
                log::error!("Lost DRM master (another process took control of display)");
            }

            use_front = !use_front;
        }
        log::debug!("Display worker: channel closed, exiting");

        if let Err(e) = card.destroy_framebuffer(fb_front) {
            log::warn!("Failed to destroy front framebuffer: {}", e);
        }
        if let Err(e) = card.destroy_framebuffer(fb_back) {
            log::warn!("Failed to destroy back framebuffer: {}", e);
        }
        if let Err(e) = card.destroy_dumb_buffer(dumb_buffer_front) {
            log::warn!("Failed to destroy front dumb buffer: {}", e);
        }
        if let Err(e) = card.destroy_dumb_buffer(dumb_buffer_back) {
            log::warn!("Failed to destroy back dumb buffer: {}", e);
        }

        log::info!("Display worker thread terminated");
    }

    fn create_dumb_buffer(card: &Card, width: u32, height: u32) -> DumbBuffer {
        use drm::buffer::DrmFourcc;

        card.create_dumb_buffer((width, height), DrmFourcc::Xrgb8888, 32)
            .expect("Failed to create dumb buffer")
    }

    fn create_framebuffer(card: &Card, buffer: &DumbBuffer) -> framebuffer::Handle {
        card.add_framebuffer(buffer, 24, 32)
            .expect("Failed to create framebuffer")
    }

    /// Converts `command` to XRGB8888 and writes it into `dumb_buffer`.
    ///
    /// `row` is a scratch buffer of `width * 4` bytes.
    fn copy_to_dumb_buffer(
        card: &Card,
        dumb_buffer: &mut DumbBuffer,
        command: &DisplayCommand,
        row: &mut [u8],
    ) -> Result<(), String> {
        let (width, height) = (command.width as usize, command.height as usize);
        let src_stride = width * 3;
        let dst_stride = width * 4;

        if command.rgb.len() != src_stride * height {
            return Err(format!(
                "Pixel data size mismatch: expected {}, got {}",
                src_stride * height,
                command.rgb.len()
            ));
        }
        if row.len() != dst_stride {
            return Err(format!(
                "Frame width {} does not match the display width",
                width
            ));
        }

        // The hardware row pitch may be larger than `width * 4`.
        let pitch = dumb_buffer.pitch() as usize;
        let mut mapping = card
            .map_dumb_buffer(dumb_buffer)
            .map_err(|e| format!("Failed to map dumb buffer: {}", e))?;
        let dst = mapping.as_mut();

        if dst.len() < pitch * height {
            return Err(format!(
                "Dumb buffer too small: {} bytes for {} rows of pitch {}",
                dst.len(),
                height,
                pitch
            ));
        }

        for (y, src_row) in command.rgb.chunks_exact(src_stride).enumerate() {
            rgb_to_xrgb_row(src_row, row);
            dst[y * pitch..y * pitch + dst_stride].copy_from_slice(row);
        }

        Ok(())
    }
}

/// Expands a row of RGB pixels into XRGB8888 (little-endian memory order B, G, R, X).
fn rgb_to_xrgb_row(rgb: &[u8], xrgb: &mut [u8]) {
    for (src, dst) in rgb.chunks_exact(3).zip(xrgb.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = 0xFF;
    }
}

impl Drop for DisplayThread {
    fn drop(&mut self) {
        // Dropping the sender closes the channel, which makes the worker leave its
        // receive loop; it must happen before `join`.
        self.sender.take();

        if let Some(handle) = self.thread_handle.take() {
            if let Err(e) = handle.join() {
                log::error!("Failed to join display thread: {:?}", e);
            }
        }

        log::info!("Display thread shut down");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_to_xrgb_row_reorders_channels() {
        let rgb = [1, 2, 3, 10, 20, 30];
        let mut xrgb = [0u8; 8];

        rgb_to_xrgb_row(&rgb, &mut xrgb);

        // B, G, R, X for each pixel.
        assert_eq!(xrgb, [3, 2, 1, 0xFF, 30, 20, 10, 0xFF]);
    }

    #[test]
    fn rgb_to_xrgb_row_handles_empty_row() {
        let mut xrgb: [u8; 0] = [];

        rgb_to_xrgb_row(&[], &mut xrgb);
    }
}
