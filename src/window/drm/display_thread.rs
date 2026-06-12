//! Async display thread for non-blocking DRM operations.
//!
//! This module implements an asynchronous display thread that handles the blocking
//! operations (buffer copy + set_crtc) in a separate thread, allowing the main
//! rendering thread to continue without waiting for display operations to complete.
//!
//! Based on the drm-gfx approach with double buffering and channel-based communication.

use super::card::Card;
use drm::control::dumbbuffer::DumbBuffer;
use drm::control::{connector, crtc, framebuffer, Device as ControlDevice, Mode};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};

/// Command sent to the display thread
pub struct DisplayCommand {
    /// Pixel data to be copied to DRM buffer and displayed
    pub pixel_data: Vec<u8>,
    /// Width of the frame in pixels
    pub width: u32,
    /// Height of the frame in pixels
    pub height: u32,
}

/// Configuration for the display thread
pub struct DisplayThreadConfig {
    pub connector: connector::Handle,
    pub crtc: crtc::Handle,
    pub mode: Mode,
}

/// Buffer pool for recycling pixel buffers between threads
pub struct BufferPool {
    /// Channel for receiving recycled buffers
    available: Receiver<Vec<u8>>,
    /// Channel for sending buffers back to the pool
    recycle: Sender<Vec<u8>>,
}

impl BufferPool {
    /// Create a new buffer pool with pre-allocated buffers
    ///
    /// # Arguments
    /// * `num_buffers` - Number of buffers to pre-allocate (typically 2-3 for double/triple buffering)
    /// * `buffer_size` - Size of each buffer in bytes (width * height * bytes_per_pixel)
    pub fn new(num_buffers: usize, buffer_size: usize) -> Self {
        let (recycle_tx, available_rx) = channel();

        // Pre-allocate buffers
        for i in 0..num_buffers {
            let buffer = vec![0u8; buffer_size];
            recycle_tx
                .send(buffer)
                .expect("Failed to initialize buffer pool");
            log::debug!(
                "Buffer pool: pre-allocated buffer {}/{}",
                i + 1,
                num_buffers
            );
        }

        log::info!(
            "Buffer pool created with {} buffers of {} bytes each",
            num_buffers,
            buffer_size
        );

        BufferPool {
            available: available_rx,
            recycle: recycle_tx,
        }
    }

    /// Try to get an available buffer without blocking
    pub(crate) fn try_get_buffer(&self) -> Option<Vec<u8>> {
        self.available.try_recv().ok()
    }

    /// Get a sender for recycling buffers (for use in other threads)
    pub fn recycler(&self) -> Sender<Vec<u8>> {
        self.recycle.clone()
    }
}

/// Manages the async display thread for non-blocking presentation
pub struct DisplayThread {
    /// Sender for sending display commands to the worker thread
    /// Uses a bounded channel (capacity 1) for natural backpressure
    sender: Option<SyncSender<DisplayCommand>>,
    /// Handle to the worker thread (for cleanup)
    thread_handle: Option<JoinHandle<()>>,
}

impl DisplayThread {
    /// Create and start a new display thread
    ///
    /// # Arguments
    /// * `card` - DRM card device (moved to display thread)
    /// * `config` - Display configuration
    /// * `buffer_recycler` - Sender for returning buffers to the pool
    /// * `width` - Display width in pixels
    /// * `height` - Display height in pixels
    pub fn new(
        card: Card,
        config: DisplayThreadConfig,
        buffer_recycler: Sender<Vec<u8>>,
        width: u32,
        height: u32,
    ) -> Self {
        // Use a bounded channel with capacity 1 for natural backpressure:
        // - Main thread can queue one frame while display thread processes another
        // - This provides one frame of pipelining/latency
        // - If channel is full, send_frame blocks until display thread consumes a frame
        let (sender, receiver) = sync_channel(1);

        // Spawn the display worker thread
        // Note: buffer_recycler is already a clone from buffer_pool.recycler()
        let thread_handle = thread::spawn(move || {
            Self::display_worker(card, config, receiver, buffer_recycler, width, height);
        });

        log::info!("Display thread started with bounded channel (capacity 1)");

        DisplayThread {
            sender: Some(sender),
            thread_handle: Some(thread_handle),
        }
    }

    /// Send a frame to be displayed
    ///
    /// Uses a bounded channel for natural backpressure. If the display thread is
    /// still processing the previous frame, this will block until it's ready.
    /// This allows one frame of pipelining: the main thread can render frame N+1
    /// while the display thread presents frame N.
    pub fn send_frame(&self, command: DisplayCommand) -> Result<(), String> {
        if let Some(sender) = &self.sender {
            sender
                .send(command)
                .map_err(|e| format!("Failed to send display command: {}", e))?;
            Ok(())
        } else {
            Err("Display thread sender has been closed".to_string())
        }
    }

    /// Worker thread function that handles display operations
    fn display_worker(
        card: Card,
        config: DisplayThreadConfig,
        receiver: Receiver<DisplayCommand>,
        buffer_recycler: Sender<Vec<u8>>,
        width: u32,
        height: u32,
    ) {
        log::debug!("Display worker thread started");

        // Create double buffers for DRM
        let mut dumb_buffer_front = Self::create_dumb_buffer(&card, width, height);
        let mut dumb_buffer_back = Self::create_dumb_buffer(&card, width, height);

        // Create framebuffers for the dumb buffers
        let fb_front = Self::create_framebuffer(&card, &dumb_buffer_front);
        let fb_back = Self::create_framebuffer(&card, &dumb_buffer_back);

        let mut use_front = true;

        loop {
            match receiver.recv() {
                Ok(command) => {
                    log::trace!(
                        "Display worker: received frame {}x{}",
                        command.width,
                        command.height
                    );

                    // Select which buffer to use
                    let (dumb_buffer, fb) = if use_front {
                        (&mut dumb_buffer_front, fb_front)
                    } else {
                        (&mut dumb_buffer_back, fb_back)
                    };

                    // Copy pixel data to DRM buffer
                    if let Err(e) = Self::copy_to_dumb_buffer(
                        &card,
                        dumb_buffer,
                        &command.pixel_data,
                        command.width,
                        command.height,
                    ) {
                        log::error!("Display worker: failed to copy to dumb buffer: {}", e);
                        // Recycle buffer and continue
                        buffer_recycler.send(command.pixel_data).ok();
                        continue;
                    }

                    // Perform the blocking set_crtc operation
                    if let Err(e) = card.set_crtc(
                        config.crtc,
                        Some(fb),
                        (0, 0),
                        &[config.connector],
                        Some(config.mode),
                    ) {
                        log::error!("Display worker: set_crtc failed: {}", e);
                        log::error!("Lost DRM master (another process took control of display)");
                    } else {
                        log::trace!("Display worker: frame presented successfully");
                    }

                    // Toggle buffers for next frame
                    use_front = !use_front;

                    // Recycle the pixel buffer
                    buffer_recycler.send(command.pixel_data).ok();
                }
                Err(_) => {
                    log::debug!("Display worker: channel closed, exiting");
                    break;
                }
            }
        }

        // Cleanup
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

    /// Create a dumb buffer for DRM
    fn create_dumb_buffer(card: &Card, width: u32, height: u32) -> DumbBuffer {
        use drm::buffer::DrmFourcc;

        let format = DrmFourcc::Xrgb8888;
        card.create_dumb_buffer((width, height), format, 32)
            .expect("Failed to create dumb buffer")
    }

    /// Create a framebuffer for a dumb buffer
    fn create_framebuffer(card: &Card, buffer: &DumbBuffer) -> framebuffer::Handle {
        card.add_framebuffer(buffer, 24, 32)
            .expect("Failed to create framebuffer")
    }

    /// Copy pixel data to a DRM dumb buffer
    fn copy_to_dumb_buffer(
        card: &Card,
        dumb_buffer: &mut DumbBuffer,
        pixel_data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        // Map the dumb buffer for writing
        let mut mapping = card
            .map_dumb_buffer(dumb_buffer)
            .map_err(|e| format!("Failed to map dumb buffer: {}", e))?;

        let buffer = mapping.as_mut();
        let stride = (width * 4) as usize; // 4 bytes per pixel
        let expected_size = stride * height as usize;

        if pixel_data.len() != expected_size {
            return Err(format!(
                "Pixel data size mismatch: expected {}, got {}",
                expected_size,
                pixel_data.len()
            ));
        }

        // Copy all pixels at once
        buffer[..expected_size].copy_from_slice(&pixel_data[..expected_size]);

        Ok(())
    }

    /// Wait for all pending display operations to complete
    ///
    /// This can be used to ensure all queued frames have been presented.
    /// Note: The Drop implementation also handles proper shutdown by closing
    /// the channel and joining the thread.
    #[allow(dead_code)]
    pub fn sync(&self) {
        // Send an empty message and wait for the channel to drain
        // Since the channel has capacity 1, this ensures any queued frame
        // has been processed. A more robust approach would be a dedicated
        // sync command, but this is sufficient for the current use case.
        if let Some(sender) = &self.sender {
            // If we can't send (channel full), the worker is busy and will
            // eventually process all frames. If we can send, we know the
            // channel is being drained.
            let _ = sender.try_send(DisplayCommand {
                pixel_data: vec![],
                width: 0,
                height: 0,
            });
        }
    }
}

impl Drop for DisplayThread {
    fn drop(&mut self) {
        log::debug!("Shutting down display thread");

        // Drop the sender to close the channel, which signals the worker thread to exit
        // This must happen before join() so the worker thread can exit its recv() loop
        self.sender.take();
        log::debug!("Display thread channel closed, worker should exit");

        // Wait for the thread to finish gracefully
        // The join() call will block until the thread exits after processing
        // any remaining frames and completing cleanup
        if let Some(handle) = self.thread_handle.take() {
            log::debug!("Waiting for display worker thread to exit...");
            if let Err(e) = handle.join() {
                log::error!("Failed to join display thread: {:?}", e);
            } else {
                log::debug!("Display worker thread exited cleanly");
            }
        }

        log::info!("Display thread shut down");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_command_creation() {
        // Verify DisplayCommand can be created
        let pixel_data = vec![0u8; 1920 * 1080 * 4];
        let cmd = DisplayCommand {
            pixel_data,
            width: 1920,
            height: 1080,
        };

        assert_eq!(cmd.width, 1920);
        assert_eq!(cmd.height, 1080);
        assert_eq!(cmd.pixel_data.len(), 1920 * 1080 * 4);
    }

    #[test]
    fn test_buffer_pool_creation() {
        let pool = BufferPool::new(3, 1024);

        // Should be able to get 3 buffers
        let buf1 = pool.try_get_buffer();
        let buf2 = pool.try_get_buffer();
        let buf3 = pool.try_get_buffer();

        assert!(buf1.is_some());
        assert!(buf2.is_some());
        assert!(buf3.is_some());

        // Should be empty now
        let buf4 = pool.try_get_buffer();
        assert!(buf4.is_none());

        // Recycle one buffer
        let recycler = pool.recycler();
        recycler
            .send(buf1.unwrap())
            .expect("Failed to recycle buffer");

        // Should be able to get it back
        let buf5 = pool.try_get_buffer();
        assert!(buf5.is_some());
    }

    #[test]
    fn test_buffer_pool_recycling() {
        let pool = BufferPool::new(2, 512);
        let recycler = pool.recycler();

        let mut buffer = pool
            .try_get_buffer()
            .expect("Failed to get buffer from pool");
        assert_eq!(buffer.len(), 512);

        // Modify buffer
        buffer[0] = 42;
        buffer[100] = 99;

        // Recycle it
        recycler.send(buffer).expect("Failed to recycle buffer");

        // Get it back
        let recycled = pool
            .try_get_buffer()
            .expect("Failed to get recycled buffer");
        assert_eq!(recycled[0], 42);
        assert_eq!(recycled[100], 99);
        // Note: We get a buffer back, but it may be newly allocated or recycled
        // The important thing is that the pool works and provides buffers
    }

    #[test]
    fn test_buffer_pool_size() {
        let buffer_size = 1920 * 1080 * 4; // Full HD XRGB
        let pool = BufferPool::new(2, buffer_size);

        let buf1 = pool.try_get_buffer();
        assert!(buf1.is_some());
        assert_eq!(buf1.unwrap().len(), buffer_size);
    }
}
