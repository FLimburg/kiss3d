Compressed Context

### Repository Snapshot
- **Project**: `kiss3d_fl` — a fork of `dimforge/kiss3d` on branch `sync`, merging `upstream/master` (v0.44.0) into HEAD (which is FLimburg/kiss3d with DRM support).
- **Merge state**: in-progress (`git merge` not yet committed). Many files already resolved/staged by the user; I've been resolving remaining ones.

### Two Architectural Threads Being Merged

**HEAD (FLimburg/kiss3d) added**: DRM/KMS headless backend
- `#[cfg(feature = "drm")] mod drm;` in `src/window/mod.rs` → contains `DrmCanvas`, `DrmSurfaceTexture<'a>`, `DrmEventManager`, `DrmCanvasError`, display thread, dumb buffer mgmt.
- Cross-backend `Window` struct in `src/window/window_common.rs` with `#[cfg]`-gated `canvas: Canvas` vs `canvas: DrmCanvas` and `events` vs `event_manager`. Holds only **basic** fields (no HDR/SSAO/etc.).
- `src/window/window.rs` has `#![cfg(not(feature = "drm"))]` — only compiled for non-DRM.
- `CanvasInputState<'a>` abstraction so camera traits don't need a full `&Canvas`. All camera traits (`Camera2d`, `Camera3d` in `src/camera/{camera2d,camera3d}.rs`) use `&CanvasInputState<'_>`.
- DRM-specific API differences: `DrmCanvas::get_current_texture() -> Result<DrmSurfaceTexture, DrmCanvasError>` vs `Canvas::get_current_texture() -> Option<wgpu::SurfaceTexture>`; same for `present`/`copy_frame_to_readback`.

**upstream/master v0.44.0 added** (massive feature set):
- Advanced rasterizer: `HdrPipeline` (MSAA HDR film + tonemap), `ShadowMapper`, `Skybox` (IBL), `Ssao`, `Ssr`, `Dof`, `Transmission` (refractive glass), `Clustered` (forward+ lighting), `ReflectionProbes` + `ProbeCapture`, `AovRenderer`, OIT.
- `RayTracer` + `rt_switcher` feature for path-tracing switching.
- Timing: `CpuTimer`, `GpuTimer`, `RenderTimings`, `last_frame_instant`.
- Surface acquisition retry (`STARTUP_SURFACE_TIMEOUT`, `acquire_next_frame`, `first_frame`).
- Hidden-window offscreen rendering via `offscreen_output_target`.
- Reflector/mirror rendering (separate frame method).
- Camera traits changed to `&Canvas` instead of `&CanvasInputState` (upstream only).
- New deps: `gltf`, `rfd`, `wesl`, bumped to `egui 0.34`, `wgpu 29`, `glamx 0.3`, `rand 0.10`.

### Done So Far (4 files resolved)

1. **`src/camera/fixed_view2d.rs`**: Took upstream's improved impl (coordinate systems, hidpi handling) BUT kept HEAD's `&CanvasInputState<'_>` trait signature (used `input.scale_factor` not `canvas.scale_factor()`) — to match the project's still-`CanvasInputState`-based `Camera2d` trait.

2. **`src/window/rendering.rs`**: Took upstream for all 7 conflicts (drops HEAD's `render_frame_3d` helper + `#[cfg(feature="drm")]` texture guards). Removed orphaned `present(frame)` between conflicts 5/6 (upstream handles present with `Option`-aware logic). Converted all `&self.canvas` camera-trait calls (`handle_event`/`update`/`start_pass`/`render_complete`) to `&input` to match project's `CanvasInputState` traits; added `let input = self.canvas.input_state();` in `raytrace_3d_frame`.

3. **`src/window/window.rs`**: Took upstream for all 4 conflicts → defines full upstream `Window` struct here (with `hdr`, `shadow_mapper`, `skybox`, `ssao`, `clustered`, `reflection_probes`, `probe_capture`, `pending_probe_captures`, `reflection_capture_layers`, `ssr`, `dof`, `transmission`, `post_process_render_target`, `offscreen_output_target`, `aov_renderer`, `hidden`, `first_frame`, `close_key`, `close_modifiers`, `last_timings`, `last_frame_instant`, `gpu_timer`, `raytracer`, etc.). This file has `#![cfg(not(feature = "drm"))]`.

4. **`src/window/mod.rs`**: Updated to conditionally re-export `Window`:
   ```rust
   #[cfg(not(feature = "drm"))] pub use window::Window;
   #[cfg(feature = "drm")]      pub use window_common::Window;
   ```
   Also re-added `CanvasInputState` to public exports.

5. **`Cargo.toml`**: Resolved 2 conflicts. Final: upstream's modern deps (`egui 0.34`, `wgpu 29`, `glamx 0.3`, `gltf`, `rfd`, `wesl`, `rand 0.10`, `default-features=false` on `image`) + HEAD's `drm = ["dep:drm"]` feature + `drm = { version = "0.14.1", optional = true }` dep. Dropped vestigial HEAD items (`parry`, `parry3d`, `parry2d`, `either`, `[[example]] decomp`, `[[example]] procedural`) — confirmed unused via grep.

### Current Known Build Errors (20 errors from `cargo check --lib --no-default-features`)

All downstream of the architectural overlap between upstream's `window.rs::Window` and HEAD's `window_common.rs::Window`:

- **`window_common.rs::Window` has methods that `window::Window` lacks**: `close()`, `set_background_color()`, … called from `src/window/events.rs:92,118` and `src/window/offscreen.rs:247`.
- **Missing imports in `window.rs`**: `Rc`, `RefCell`, `Receiver`, `CanvasInputState` are referenced but not imported (probably came in via HEAD's slim header that was discarded when I took upstream's import block).
- **Trait signature mismatches** (`E0053` on `handle_event`/`update`): camera impls in `src/camera/{orbit3d, sidescroll2d, first_person3d, first_person_stereo3d, fixed_view3d, camera3d}.rs` — same `CanvasInputState` vs `Canvas` issue I already fixed in `fixed_view2d.rs`. Likely from unresolved conflict markers (`sidescroll2d.rs` had visible markers per earlier `git diff --check`) or upstream-only changes that didn't auto-resolve.
- **DRM-specific errors in `rendering.rs`** (6 errors at lines 1174, 1195, 1416, 1434, 1475, 1496): `wgpu::SurfaceTexture` vs `DrmSurfaceTexture` mismatches in `copy_frame_to_readback`/`present`/`get_current_texture` — only fire when the `drm` feature is enabled, but rust-analyzer surfaces them.
- **Misc**: `cannot borrow *self as mutable because also borrowed as immutable` (E0502), `cannot borrow scene as mutable, not declared mut` (E0596), `E0308 mismatched types`, `E0425/E0433 cannot find type Rc/RefCell/…`.

### Two New Open Todos (you just asked about these)

**Todo A — Port missing methods + add missing imports to `window.rs`** so `events.rs` and `offscreen.rs` compile:
- Bring `close()`, `set_background_color()`, and any other methods currently only on `window_common::Window` over to the upstream `window::Window` in `src/window/window.rs`.
- Add the missing `use` imports (`std::rc::Rc`, `std::cell::RefCell`, `std::sync::mpsc::Receiver`, `crate::window::CanvasInputState`) — they were in HEAD's import block I dropped.

**Todo B — Align remaining camera trait impls with `&CanvasInputState<'_>`** (mirroring the `fixed_view2d.rs` fix):
- Check/resolve conflict markers and signatures in `src/camera/sidescroll2d.rs`, `src/camera/orbit3d.rs`, `src/camera/first_person3d.rs`, `src/camera/first_person_stereo3d.rs`, `src/camera/fixed_view3d.rs`, `src/camera/camera3d.rs`. Keep HEAD's `&CanvasInputState<'_>` signatures but absorb any upstream implementation improvements.

### DRM Implications Going Forward

- The `drm` feature is currently **structurally orphaned from upstream's new render pipeline**: `window_common.rs::Window` (used in DRM builds) lacks HDR/SSAO/shadow/etc. fields, and `rendering.rs` uses them unconditionally. A DRM build will fail to compile against `rendering.rs` regardless of Todos A/B.
- Long-term, DRM support against the new pipeline needs one of: (1) drop DRM, (2) move all upstream rendering features into `window_common.rs::Window` and adapt the DRM canvas API to match (`DrmSurfaceTexture` abstraction etc.), or (3) gate the advanced effects behind `#[cfg(not(feature = "drm"))]`.
- For the immediate non-DRM build, my `mod.rs` re-export makes `crate::window::Window = window::Window` (full upstream struct), so Todos A+B should get a non-DRM build green without touching DRM architecture.
