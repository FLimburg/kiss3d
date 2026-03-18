//! Shared utilities for canvas implementations.
//!
//! This module contains common functionality used by both windowed (WgpuCanvas)
//! and headless (DrmCanvas) implementations to avoid code duplication.

use crate::context::Context;

/// Utilities for pixel reading and format conversion.
pub mod pixel_reading {
    use super::*;

    /// Calculate the padded bytes per row for texture copying.
    ///
    /// wgpu requires texture copy operations to have rows aligned to
    /// `COPY_BYTES_PER_ROW_ALIGNMENT` (256 bytes).
    ///
    /// # Arguments
    /// * `width` - Width in pixels
    /// * `bytes_per_pixel` - Bytes per pixel (typically 4 for RGBA/BGRA)
    ///
    /// # Returns
    /// The padded row size in bytes
    #[inline]
    pub fn calculate_padded_bytes_per_row(width: usize, bytes_per_pixel: usize) -> usize {
        let unpadded = width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        unpadded.div_ceil(align) * align
    }

    /// Reads pixels from a texture into a CPU buffer as RGB data.
    ///
    /// This is the core pixel reading implementation shared by all canvas types.
    /// It handles staging buffer creation, texture copying, GPU synchronization,
    /// format conversion, and row padding.
    ///
    /// # Arguments
    /// * `texture` - The GPU texture to read from
    /// * `out` - Output buffer (will be cleared and filled with RGB data)
    /// * `x` - Left coordinate of region to read
    /// * `y` - Top coordinate of region to read
    /// * `width` - Width of region to read
    /// * `height` - Height of region to read
    /// * `format` - The texture format (used for BGRA vs RGBA detection)
    /// * `label_prefix` - Prefix for debug labels (e.g., "screenshot" or "drm_screenshot")
    ///
    /// # Output Format
    /// - RGB data (3 bytes per pixel)
    /// - Bottom-to-top row order (OpenGL convention)
    /// - No padding between rows
    pub fn read_texture_to_rgb(
        texture: &wgpu::Texture,
        out: &mut Vec<u8>,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        format: wgpu::TextureFormat,
        label_prefix: &str,
    ) {
        let ctxt = Context::get();

        // Calculate buffer size with alignment
        let bytes_per_pixel = 4; // RGBA or BGRA
        let padded_bytes_per_row = calculate_padded_bytes_per_row(width, bytes_per_pixel);
        let buffer_size = padded_bytes_per_row * height;

        // Create staging buffer for GPU -> CPU transfer
        let staging_buffer = ctxt.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{}_staging_buffer", label_prefix)),
            size: buffer_size as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy from texture to staging buffer
        let mut encoder =
            ctxt.create_command_encoder(Some(&format!("{}_copy_encoder", label_prefix)));

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
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

        // Convert from BGRA/RGBA to RGB and handle row padding
        let rgb_size = width * height * 3;
        out.clear();
        out.reserve(rgb_size);

        // Detect if we're dealing with BGRA or RGBA format
        let is_bgra = matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );

        // wgpu has origin at top-left, but we want bottom-left origin for OpenGL compatibility
        // So we read rows in reverse order
        for row in (0..height).rev() {
            let row_start = row * padded_bytes_per_row;
            for col in 0..width {
                let pixel_start = row_start + col * bytes_per_pixel;
                if is_bgra {
                    // BGRA -> RGB
                    out.push(data[pixel_start + 2]); // R
                    out.push(data[pixel_start + 1]); // G
                    out.push(data[pixel_start]); // B
                } else {
                    // RGBA -> RGB
                    out.push(data[pixel_start]); // R
                    out.push(data[pixel_start + 1]); // G
                    out.push(data[pixel_start + 2]); // B
                }
            }
        }

        drop(data);
        staging_buffer.unmap();
    }

    /// Reads pixels from a texture into a CPU buffer as RGBA data (with padding handled).
    ///
    /// This is used internally by DrmCanvas for display operations where we need
    /// to preserve the alpha channel and handle row padding.
    ///
    /// # Arguments
    /// * `texture` - The GPU texture to read from
    /// * `buffer` - Output buffer (will be resized to width × height × 4)
    /// * `width` - Width in pixels
    /// * `height` - Height in pixels
    ///
    /// # Output Format
    /// - RGBA/BGRA data (4 bytes per pixel)
    /// - Top-to-bottom row order (native GPU order)
    /// - No padding between rows in output
    pub fn read_texture_to_buffer(
        texture: &wgpu::Texture,
        buffer: &mut Vec<u8>,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let ctxt = Context::get();

        // Calculate buffer layout with proper alignment
        let bytes_per_pixel = 4;
        let stride = (width * 4) as usize;
        let padded_bytes_per_row = calculate_padded_bytes_per_row(width as usize, bytes_per_pixel);
        let buffer_size = padded_bytes_per_row * height as usize;

        // Create staging buffer for GPU -> CPU transfer
        let staging_buffer = ctxt.create_buffer(&wgpu::BufferDescriptor {
            label: Some("texture_read_staging"),
            size: buffer_size as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy texture to staging buffer
        let mut encoder = ctxt.create_command_encoder(Some("texture_read_encoder"));
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
            .map_err(|e| format!("Failed to receive buffer map result: {}", e))?
            .map_err(|e| format!("Failed to map staging buffer: {}", e))?;

        let mapped_data = buffer_slice.get_mapped_range();

        // Resize output buffer if needed
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

        Ok(())
    }
}

/// Utilities for creating GPU textures.
pub mod texture_utils {
    use super::*;

    /// Creates a depth texture for rendering.
    ///
    /// # Arguments
    /// * `device` - The wgpu device
    /// * `width` - Width in pixels (minimum 1)
    /// * `height` - Height in pixels (minimum 1)
    /// * `sample_count` - Number of MSAA samples (minimum 1)
    ///
    /// # Returns
    /// A tuple of (texture, texture_view)
    pub fn create_depth_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        sample_count: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let sample_count = sample_count.max(1);
        let width = width.max(1);
        let height = height.max(1);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: Context::depth_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// Creates an MSAA texture for multisampled rendering.
    ///
    /// # Arguments
    /// * `device` - The wgpu device
    /// * `width` - Width in pixels (minimum 1)
    /// * `height` - Height in pixels (minimum 1)
    /// * `format` - The texture format
    /// * `sample_count` - Number of MSAA samples
    ///
    /// # Returns
    /// A tuple of (texture, texture_view)
    pub fn create_msaa_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let width = width.max(1);
        let height = height.max(1);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// Creates a readback texture for CPU-side pixel reading.
    ///
    /// This texture is used as an intermediate for copying rendered frames
    /// before reading them back to CPU memory (e.g., for screenshots).
    ///
    /// # Arguments
    /// * `device` - The wgpu device
    /// * `width` - Width in pixels (minimum 1)
    /// * `height` - Height in pixels (minimum 1)
    /// * `format` - The texture format
    /// * `label` - Debug label for the texture
    ///
    /// # Returns
    /// A texture configured for copy operations
    pub fn create_readback_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> wgpu::Texture {
        let width = width.max(1);
        let height = height.max(1);

        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
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
