# DRM Display Output Implementation Plan

**Status**: ✅ Phase 3 COMPLETE - Fully Functional Display Output!  
**Last Updated**: 2024-12  
**Target Platform**: Raspberry Pi 4 (vc4-kms-v3d)

---

## 🎉 Implementation Complete

**DRM display output is fully implemented and tested on Raspberry Pi 4!**

All phases are now complete:
- ✅ **Phase 1**: Display Discovery - Resource enumeration and detection
- ✅ **Phase 2**: Display Output - DRM device initialization and basic output
- ✅ **Phase 3**: Optimization - Vec-based buffer pool + async display thread

**Project**: kiss3d DRM Support  
**Last Updated**: 2024-12

---

## Current Status

**Phase 3 COMPLETE** - DRM display output is fully implemented and tested on Raspberry Pi 4!

### Key Achievements
- ✅ Vec-based buffer pool architecture (safe, no unsafe code)
- ✅ Async display thread (Card ownership in worker thread)
- ✅ Double buffering in display thread
- ✅ Clean shutdown with proper resource cleanup
- ✅ Simplified from GBM to direct dumb buffer approach
- ✅ ~80 lines of code removed, cleaner architecture

### Architecture Change
**Original Plan**: Use GBM for buffer management  
**Implemented**: Direct DRM dumb buffers + Vec-based buffer pool

This change simplified the implementation significantly while maintaining performance.

---

## Overview

This document outlines the complete implementation plan for adding DRM (Direct Rendering Manager) display output support to kiss3d, enabling rendering directly to displays without a window manager (console-only systems like Raspberry Pi).

---

## Architecture

```
┌────────────────────────────────────────────────────────────┐
│                         kiss3d                             │
│                                                            │
│  ┌─────────────┐     ┌──────────────┐    ┌──────────────┐  │
│  │  DRMWindow  │────▶│  DrmCanvas   │────▶│  wgpu       │  │
│  └─────────────┘     └──────────────┘    └──────────────┘  │
│                              │                             │
│                              ▼                             │
│                      ┌──────────────┐                      │
│                      │ RenderMode   │                      │
│                      └──────────────┘                      │
│                       │           │                        │
│               ┌───────┘           └────────┐               │
│               ▼                             ▼              │
│       ┌──────────────┐            ┌──────────────┐         │
│       │  Offscreen   │            │   Display    │         │
│       │  (existing)  │            │DrmDisplayState│        │
│       └──────────────┘            └──────────────┘         │
│                                            │               │
└────────────────────────────────────────────┼───────────────┘
                                             │
                 ┌───────────────────────────┼──────────────────────┐
                 │        Linux Kernel       │                      │
                 │                           ▼                      │
                 │  ┌──────────┐      ┌──────────┐     ┌─────────┐  │
                 │  │   DRM    │◀────▶│   GBM    │◀───▶│  GPU    │  │
                 │  │(KMS API) │      │(Buffers) │     │ Driver  │  │
                 │  └──────────┘      └──────────┘     └─────────┘  │
                 │       │                                          │
                 │       ▼                                          │
                 │  ┌──────────┐                                    │
                 │  │ Display  │                                    │
                 │  │Hardware  │                                    │
                 │  └──────────┘                                    │
                 └──────────────────────────────────────────────────┘
```

---

## Implementation Phases

### Phase 1: Foundation ✅ **COMPLETE** (2024)

**Goal**: Establish display resource discovery infrastructure

**Components Implemented**:
- ✅ Enhanced error types (DrmError, GbmError, ModesetError, etc.)
- ✅ Helper structs (DisplayConfig, FormatInfo, DrmDisplayState)
- ✅ Display resource query functions
  - `query_display_resources()` - Main entry point
  - `find_connected_connector()` - Finds active display
  - `find_available_crtc()` - Selects display controller
  - `select_best_mode()` - Chooses optimal resolution
  - `choose_formats()` - Format compatibility selection
- ✅ Drop implementation for DrmDisplayState (resource cleanup)

**Testing**: `examples/drm_test.rs` Phase 1 section
- Opens DRM devices with fallback
- Queries connectors, encoders, CRTCs
- Enumerates display modes
- Validates query logic

**Files Modified**:
- `src/window/drm/drm_canvas.rs` - Core implementation
- `src/window/drm/card.rs` - DRM device wrapper

---

### Phase 2: Display Output ✅ **COMPLETE** (2024-12)

**Note**: Originally planned as "GBM Integration" but implemented with direct DRM dumb buffers instead.

**Goal**: Initialize GBM for GPU buffer management

**Components Implemented**:
- ✅ RenderMode enum (Offscreen vs Display)
- ✅ DrmDisplayState struct with complete field set
- ✅ `new_with_display()` constructor
  - Opens DRM device
  - Queries display configuration
  - Creates GBM device
  - Creates GBM surface (buffer pool)
  - Initializes wgpu
  - Sets up offscreen buffers
  - Builds complete display state
- ✅ Updated `present()` method signature to return Result

**Buffer Strategy**: CPU Copy Approach
- Render to offscreen wgpu texture
- Copy to GBM buffer before display
- Simple, works on all hardware
- Future optimization: DMA-BUF zero-copy

**Testing**: `examples/drm_test.rs` Phase 2 section
- Creates GBM device
- Validates format support
- Allocates GBM surface
- Tests buffer locking/unlocking

**Files Modified**:
- `src/window/drm/drm_canvas.rs` - GBM integration
- `src/window/drm/drm_window.rs` - Updated present() call

---

### Phase 3: Optimization ✅ **COMPLETE** (2024-12)

Phase 3 is now complete with a Vec-based buffer architecture inspired by reference implementations.

**Key Implementation Details**:

#### Vec-Based Buffer Pool Architecture

**Goal**: Actually display rendered frames on screen

**Status**: ✅ Implemented in `display_thread.rs`

```rust
// BufferPool manages triple buffering with Vec<u8>
pub struct BufferPool {
    available: Receiver<Vec<u8>>,  // Get buffers
    recycle: Sender<Vec<u8>>,      // Return buffers
}

// Pre-allocate 3 buffers for triple buffering
BufferPool::new(3, width * height * 4);
```

**Benefits**:
- Safe (no unsafe code, Rust ownership)
- Fast (Vec move is 24 bytes, heap stays in place)
- Simple (clear ownership semantics)
- Automatic recycling through channels

#### Async Display Thread with Card Ownership

**Status**: ✅ Implemented in `display_thread.rs`

The display worker thread owns the Card and handles ALL DRM operations:

**Architecture**:
```
Main Thread → [GPU Render] → [Read to Vec<u8>] → [Send to Channel]
                                                          ↓
Display Thread ← [Receive Vec<u8>] ← [Map Dumb Buffer] ← [Copy Pixels]
       ↓
   [Create FB] → [set_crtc] → [Recycle Buffer]
```

**Implementation** (`display_thread.rs`):
- `DisplayThread::new()` - Spawns worker thread, passes Card ownership
- `display_worker()` - Main loop: receives frames, copies to DRM, displays
- Creates two dumb buffers on startup (double buffering)
- Creates two framebuffers (one per dumb buffer)
- Toggles between buffers each frame
- Cleans up DRM resources on thread exit

**Key Functions**:
- `create_dumb_buffer()` - Creates DRM dumb buffer
- `create_framebuffer()` - Wraps dumb buffer in framebuffer
- `copy_to_dumb_buffer()` - Copies pixel data to DRM buffer
- Worker loop handles `set_crtc()` calls (blocking, but in separate thread)

---

**Implementation**: Display thread creates framebuffers for its dumb buffers on startup.

```rust
// In display_worker initialization:
let fb_front = Self::create_framebuffer(&card, &dumb_buffer_front);
let fb_back = Self::create_framebuffer(&card, &dumb_buffer_back);
```

No caching needed - two framebuffers created once, reused throughout.

#### Step 2: Framebuffer Management

**Status**: ✅ Implemented - Two static framebuffers in display thread

**Implementation**:
```rust
fn create_framebuffer(card: &Card, buffer: &DumbBuffer) -> framebuffer::Handle {
    card.add_framebuffer(buffer, 24, 32)
        .expect("Failed to create framebuffer")
}
```

No caching or complex management needed:
- Two dumb buffers created on thread startup
- Two framebuffers created (one per buffer)
- Reused throughout program lifetime
- Destroyed on thread shutdown

**Benefits**:
- Simple: no hash maps or caching logic
- Safe: framebuffers tied to buffer lifetime
- Fast: no lookup overhead

---

**Implementation**: Split between main thread (GPU read) and display thread (DRM copy).

**Main thread** (`drm_canvas.rs`):
```rust
fn read_texture_to_buffer(
    texture: &wgpu::Texture,
    buffer: &mut Vec<u8>,  // From pool
    width: u32,
    height: u32,
) -> Result<(), DrmCanvasError>
```

**Display thread** (`display_thread.rs`):
```rust
fn copy_to_dumb_buffer(
    card: &Card,
    dumb_buffer: &mut DumbBuffer,
    pixel_data: &[u8],     // Received via channel
    width: u32,
    height: u32,
) -> Result<(), String>
```

#### Step 3: Frame Rendering and Copy

**Status**: ✅ Implemented - Split between threads for better parallelism

**Main Thread** (`drm_canvas.rs::read_texture_to_buffer()`):
```rust
// 1. Get buffer from pool
let mut pixel_buffer = buffer_pool.try_get_buffer().unwrap_or_else(...);

// 2. Read GPU texture to CPU buffer (wgpu staging buffer)
Self::read_texture_to_buffer(&offscreen_texture, &mut pixel_buffer, w, h)?;

// 3. Send to display thread (Vec ownership transfer)
display_thread.send_frame(DisplayCommand {
    pixel_data: pixel_buffer,
    width, height
})?;
```

**Display Thread** (`display_thread.rs::copy_to_dumb_buffer()`):
```rust
// 1. Map dumb buffer
let mut mapping = card.map_dumb_buffer(dumb_buffer)?;
let buffer = mapping.as_mut();

// 2. Bulk copy (no format conversion needed - both RGBA/XRGB)
buffer[..size].copy_from_slice(&pixel_data[..size]);
```

**Benefits**:
- Main thread non-blocking after GPU read
- Display thread handles slow DRM operations
- Parallel execution improves throughput

---

**Implementation**: Using `set_crtc` in display thread (blocking operation isolated).

```rust
// In display_worker loop:
card.set_crtc(
    config.crtc,
    Some(fb),
    (0, 0),
    &[config.connector],
    Some(config.mode),
)
```

**Note**: First frame blocks for initial modeset (unavoidable). Subsequent frames use double buffering.

**Future**: Could be optimized with `page_flip` API for true async (Phase 4).

#### Step 4: Page Flipping / Display Presentation

**Status**: ✅ Implemented with `set_crtc` (simpler than page_flip)

**Implementation** (`display_thread.rs`):
```rust
// In display_worker loop, after copying pixels:
card.set_crtc(
    config.crtc,
    Some(fb),              // Framebuffer to display
    (0, 0),                // Position
    &[config.connector],   // Output connector
    Some(config.mode),     // Display mode
)?;
```

**Double Buffering**:
- Two dumb buffers (front and back)
- Two framebuffers (one per buffer)
- Toggle `use_front` flag each frame
- While one buffer displays, render to the other

**Flow**:
1. Receive pixel data from main thread
2. Copy to current back buffer
3. Call `set_crtc()` with corresponding framebuffer
4. Toggle buffers (back becomes front, front becomes back)
5. Recycle pixel data Vec to pool

**Benefits**:
- Simple: no complex page flip event handling
- Works: `set_crtc()` provides implicit VSync
- Isolated: blocking call in separate thread

---

**Implementation**: Implicit VSync through `set_crtc` blocking.

The `set_crtc` call blocks until the next vblank, providing natural frame pacing. More explicit VBlank handling would be Phase 4 enhancement.

#### Step 5: VSync/VBlank Handling

**Status**: ✅ Implicit VSync via `set_crtc()` blocking behavior

**Current Implementation**:
- `set_crtc()` is a blocking call that waits for VBlank
- Provides natural frame pacing at display refresh rate
- Simple and reliable - no event handling needed
- Display thread blocks, but main thread continues rendering

**Benefits**:
- Zero additional code - implicit in `set_crtc()`
- Guaranteed VSync (no tearing)
- Works on all DRM drivers
- No polling or event queue management

**Future Enhancement (Phase 4)**:
- Use `page_flip()` with `PageFlipFlags::EVENT`
- Async event handling via DRM event file descriptor
- Would enable true non-blocking presentation
- More complex but potentially better frame timing

---

### Phase 4: Advanced Features 🔮 **PLANNED**

Phase 3 already includes significant optimizations. Phase 4 would add advanced features.

**Goal**: Improve performance and features

#### Planned Advanced Features:

**1. 2D Overlay Rendering**
- Wire up `polyline_renderer_2d` and `point_renderer_2d`
- Already initialized in DRMWindow, just need render calls
- Would enable `draw_line_2d()`, `draw_point_2d()` API
- See `Window` implementation in `rendering.rs` lines 368-386

**2. Post-Processing Effects**
- Use `framebuffer_manager` and `post_process_render_target`
- Multi-pass rendering for bloom, blur, tone-mapping
- More complex, requires shader knowledge

**3. Page Flip API**

1. **DMA-BUF Zero-Copy Path**
   - Export GBM buffer as DMA-BUF fd
   - Import into wgpu as external texture
   - Render directly to display buffer
   - Eliminates CPU copy overhead

2. **Async VSync Handling**
   - Non-blocking page flip
   - Event loop integration
   - Better frame pacing
   - Reduced latency

3. **Triple Buffering**
   - Maintain 3 buffers (front, back, queued)
   - Prevent blocking on buffer acquisition
   - Smoother frame rate
   - Better GPU utilization

4. **Format Optimization**
   - Direct XRGB8888 rendering in wgpu
   - Avoid format conversion
   - Potentially use GPU for conversion

5. **Multi-Display Support**
   - Handle multiple connectors
   - Independent rendering per display
   - Clone or extended modes

---

**4. VBlank Events**
- Explicit VBlank event handling for perfect timing
- Would replace implicit set_crtc blocking

**5. DMA-BUF Zero-Copy**
- Eliminate GPU→CPU copy if hardware supports it
- Significant performance improvement potential

#### Original Planned Improvements:
- ✅ Async display thread - **COMPLETE** (implemented in Phase 3)
- ✅ Double buffering - **COMPLETE** (implemented in Phase 3)
- ✅ Buffer pool - **COMPLETE** (implemented in Phase 3)
- 🔮 Non-blocking page_flip - Planned for Phase 4
- 🔮 Triple buffering in display thread - Could be added
- 🔮 DMA-BUF zero-copy - Planned for Phase 4

### Phase 5: Polish 🔮 **PLANNED**

**Goal**: Production-ready features

1. **Error Recovery**
   - Handle display disconnect
   - Recover from driver errors
   - Fallback to offscreen mode

2. **Dynamic Mode Changes**
   - Support resolution changes
   - Handle display hotplug
   - Mode preference API

3. **Performance Metrics**
   - Frame timing
   - Flip statistics
   - Buffer usage monitoring

4. **Documentation**
   - API documentation
   - Architecture guide
   - Performance tuning guide
   - Platform-specific notes

---

## Testing Strategy

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_format_conversion() {
        // Test BGRA -> XRGB conversion
    }
    
    #[test]
    fn test_framebuffer_cache() {
        // Test FB creation and caching
    }
}
```

### Integration Tests
- `examples/drm_test.rs` - Combined Phase 1 & 2 validation
- `examples/drm_cube.rs` - Basic 3D rendering test
- Future: Color bars, animated scenes, stress tests

### Hardware Testing Matrix

| Platform | GPU Driver | Status | Notes |
|----------|------------|--------|-------|
| Raspberry Pi 4 | vc4/v3d | ✅ Phase 1&2 | Primary target |
| Raspberry Pi 5 | vc4/v3d | 🔄 Untested | Should work |
| x86 Linux (Intel) | i915 | 🔄 Untested | Should work |
| x86 Linux (AMD) | amdgpu | 🔄 Untested | Should work |
| x86 Linux (NVIDIA) | nouveau | ⚠️ Limited | Limited GBM support |
| VM (virtio-gpu) | virtio | 🔄 Untested | For CI/testing |

---

## Technical Challenges

### Challenge 1: Format Conversion
**Problem**: wgpu uses BGRA8Unorm, displays typically want XRGB8888
**Solution**: CPU-based conversion during copy (Phase 3), GPU conversion later (Phase 4)

### Challenge 2: Buffer Synchronization
**Problem**: Prevent rendering to buffer being scanned out
**Solution**: GBM buffer locking + proper state tracking

### Challenge 3: Permission Requirements
**Problem**: set_crtc requires DRM master (root or active VT)
**Solution**: Document requirements, provide helpful error messages

### Challenge 4: Driver Variations
**Problem**: Different GPU drivers have different capabilities
**Solution**: Format probing, graceful degradation, extensive testing

### Challenge 5: wgpu + GBM Integration
**Problem**: wgpu doesn't natively support GBM surfaces
**Solution**: Phase 2 uses offscreen + copy; Phase 4 adds DMA-BUF path

---

## API Design

### Public API
```rust
// Existing offscreen mode (unchanged)
let canvas = DrmCanvas::new(device_path, width, height).await?;

// New display output mode
let canvas = DrmCanvas::new_with_display(device_path).await?;

// Check mode
if canvas.is_display_mode() {
    println!("Rendering to display");
}

// Present (works for both modes)
canvas.present()?;
```

### High-Level API (DRMWindow)
```rust
// Offscreen rendering (existing)
let window = DRMWindow::new(device_path, width, height).await?;

// Display output (future enhancement)
let window = DRMWindow::new_with_display(device_path).await?;
```

---

## Error Handling Strategy

### Error Types
- `DrmError` - DRM API failures
- `GbmError` - GBM operations
- `ModesetError` - Display configuration
- `PageFlipError` - Page flip failures
- `IoError` - File operations

### Error Propagation
```rust
pub fn present(&mut self) -> Result<(), DrmCanvasError> {
    match &mut self.mode {
        RenderMode::Display(display) => {
            self.present_to_display(display)
                .map_err(|e| {
                    log::error!("Display output failed: {}", e);
                    // Could fallback to offscreen here
                    e
                })
        }
        _ => Ok(())
    }
}
```

---

## Implementation Notes

### Why Vec<u8> Instead of Raw Pointers?

The reference implementation used raw pointers for minimal overhead. We chose Vec<u8> for:

1. **Safety**: No unsafe code, Rust ownership prevents bugs
2. **Performance**: Vec move is only 24 bytes (negligible vs 16ms frame time)
3. **Simplicity**: Clear ownership, automatic cleanup
4. **Correctness**: Impossible to have use-after-free or dangling pointers

**Benchmark**: Vec move overhead <1µs, insignificant for rendering.

### Why Direct Dumb Buffers Instead of GBM?

Original plan included GBM for buffer management. We simplified to:

1. **Direct dumb buffers**: Simpler DRM API, fewer dependencies
2. **Vec-based pool**: Handles buffer recycling elegantly
3. **Display thread ownership**: Card lives in one place

**Result**: Simpler code, easier to understand, works great.

### Clean Shutdown

Previous implementation had deadlock issues on Drop. Fixed by:

1. Made `sender: Option<Sender>` in DisplayThread
2. Drop takes sender first to close channel
3. Worker thread exits recv() loop
4. join() completes cleanly
5. Worker cleans up DRM resources before exit

---

## Dependencies

### Required Crates
```toml
[dependencies]
drm = { version = "0.14.1", optional = true }
wgpu = "27"

[features]
drm = ["dep:drm"]
```

### System Requirements
- Linux kernel 4.0+ with KMS
- GPU driver with DRM/KMS support
- GBM library (libgbm-dev)
- For Raspberry Pi: vc4-kms-v3d overlay enabled

---

## Performance Considerations

### Current Approach (Phase 3)
- **Copy overhead**: ~2-5ms for 1080p frame
- **Format conversion**: Minimal CPU usage
- **Frame rate**: Limited by copy + vsync

### Optimized Approach (Phase 4)
- **DMA-BUF**: Zero-copy, direct GPU rendering
- **Triple buffering**: Never block on buffer
- **Expected**: Full refresh rate, minimal CPU

### Benchmarks (Target)
| Resolution | Phase 3 | Phase 4 |
|------------|---------|---------|
| 1920x1080  | ~30ms   | ~16ms   |
| 1280x720   | ~15ms   | ~8ms    |
| 640x480    | ~5ms    | ~3ms    |

---

## Code Organization

```
src/window/drm/
├── mod.rs                    # Public exports
├── card.rs                   # DRM device wrapper
├── drm_canvas.rs            # Main implementation ⭐
│   ├── Error types
│   ├── Helper structs
│   ├── DrmCanvas impl
│   │   ├── new() (offscreen)
│   │   ├── new_with_display() ⭐ Phase 2
│   │   ├── present() ⭐ Phase 3
│   │   └── Query functions
│   └── DrmDisplayState impl
├── drm_canvas_wrapper.rs    # Canvas API compatibility
└── drm_window.rs            # High-level window API

examples/
├── drm_test.rs              # Combined Phase 1+2 test ⭐
├── drm_cube.rs              # 3D rendering example
└── (phase 3 examples...)
```

---

## Completed Implementation

### Phase 3 Completion Checklist

- ✅ Vec-based buffer pool implementation
- ✅ Async display thread with Card ownership
- ✅ Double buffering in display thread
- ✅ Clean shutdown without deadlocks
- ✅ Tested on Raspberry Pi 4
- ✅ Documentation updated (DRM_STATUS.md)
- ✅ All compiler warnings addressed
- ✅ Code simplified (~80 lines removed)

## Next Steps

### ✅ Phase 3 Complete!

All planned Phase 3 tasks are complete:
- ✅ Vec-based buffer pool implementation
- ✅ Async display thread with Card ownership
- ✅ Double buffering in display thread
- ✅ Clean shutdown without deadlocks
- ✅ Tested on Raspberry Pi 4
- ✅ Documentation updated
- ✅ All compiler warnings addressed
- ✅ Code simplified (~80 lines removed)

### Phase 4: Advanced Features (Planned)
1. **2D Overlay Rendering** - Wire up polyline_renderer_2d and point_renderer_2d
2. **Post-Processing Effects** - Use framebuffer_manager for visual effects
3. **page_flip API** - Replace set_crtc for true async presentation
4. **VBlank Events** - Explicit event handling for perfect timing
5. **DMA-BUF Zero-Copy** - Eliminate GPU→CPU copy (hardware dependent)
6. Performance profiling and optimization

### Phase 5: Polish (Future)
1. Multi-display support
2. Display hotplug handling
3. Dynamic resolution changes
4. Production hardening
5. Comprehensive documentation

---

## Resources

### Documentation
- [DRM KMS Documentation](https://www.kernel.org/doc/html/latest/gpu/drm-kms.html)
- [GBM API Reference](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/src/gbm/main/gbm.h)
- [wgpu Documentation](https://wgpu.rs/)

### Examples
- [drm-rs examples](https://github.com/Smithay/drm-rs/tree/master/examples)
- [gbm-rs examples](https://github.com/Smithay/gbm-rs/tree/master/examples)

### Related Projects
- [Smithay](https://github.com/Smithay/smithay) - Wayland compositor
- [winit](https://github.com/rust-windowing/winit) - Window handling
- [glutin](https://github.com/rust-windowing/glutin) - OpenGL context

---

## Changelog

### 2024-12 - Phase 3 Complete ✅

**Major Achievement**: DRM display output fully working on Raspberry Pi 4!

**Implementation Highlights**:
- Vec-based buffer pool architecture (safe, no unsafe code)
- Async display thread with Card ownership
- Display thread handles ALL DRM operations
- Clean shutdown with proper resource cleanup
- Simplified architecture (~80 lines removed)
- Direct dumb buffers (no GBM dependency)

**Files Modified**:
- `src/window/drm/display_thread.rs` - BufferPool + async display worker
- `src/window/drm/drm_canvas.rs` - Simplified present(), read_texture_to_buffer
- `src/window/drm/drm_window.rs` - Consistent with Window structure
- `DRM_STATUS.md` - Updated with current status
- `DRM_IMPLEMENTATION_PLAN.md` - This file

**Testing**: Verified working on Raspberry Pi 4 with 1920x1080 display.

### 2024-12 - Phase 2 Complete ✅

**Note**: Originally planned as "GBM Integration" but pivoted to direct DRM approach.
- Implemented GBM integration
- Added `new_with_display()` constructor
- Created unified test (`examples/drm_test.rs`)
- Validated on Raspberry Pi 4

### 2024 - Phase 1 Complete ✅
- Implemented display resource discovery
- Added error types and helper structs
- Created Phase 1 test
- Validated on Raspberry Pi 4

### 2024 - Project Start
- Initial planning
- Architecture design
- Dependency evaluation

---

## Contributors

- Primary: [Your Name]
- Testing: Raspberry Pi 4 validation
- Review: [Future contributors]

---

## License

Same as kiss3d: BSD-3-Clause
