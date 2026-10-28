# DRM Rebase Plan: re-create DRM on a fresh branch off upstream kiss3d

Status: **v0.5 (evdev deferred to after v1). Spike re-run on latest upstream (2026-10-27). Verdict: GO, with a smaller design than v0.3.**
Spike basis: `upstream/master` at `03727cd2` (v0.47.0, 2026-10-04), freshly fetched. v0.3 was based on `acd75153` (v0.44-era); upstream has since moved to wgpu 30 and added a public headless `Window` and pipelined readback. Old DRM code read from `origin/master` (`339de50a`).

## Executive summary

- **Problem:** the fork's DRM backend (verified on a Raspberry Pi) cannot be merged with upstream without two `Window` types and two render paths.
- **What changed upstream since v0.3 of this plan:**
  - `Window::new_headless_with_setup(width, height, setup)` is now **public**: a full `Window` with no OS window and no swapchain, working without a display server.
  - `Window::snap_begin()` / `snap_finish()` give **pipelined, non-blocking readback** (one frame in flight, persistent staging buffer).
  - wgpu is now 30; `CanvasSetup` has `required_features`; clustered lighting and SSR now fall back automatically on adapters lacking compute or buffer-binding support.
- **Recommendation:** GO. Build DRM as an **additive module that consumes the public headless API** (Tier 1). It needs no change to `rendering.rs`, `Window` or `WgpuCanvas`. Measure on the Pi. Only if the CPU cost of upstream's readback is too high, add a small generic raw-readback API (Tier 2). The in-crate render-path hook from v0.3 (Tier 3) is now a last resort.
- **Why:** `wgpu_canvas.rs` changed by about 850 lines and `rendering.rs` by about 490 between v0.44 and v0.47. A seam outside those files minimizes sync conflicts.
- **Scope decision (2026-10-27):** evdev input is secondary and handled after v1. v1 is output only (render to display via DRM/KMS), so no upstream file needs to change for input.
- **Main open risks:** (1) upstream readback returns RGB, bottom-up, then flips: extra CPU passes on a Pi; (2) Pi GPU compatibility with the HDR pipeline is unverified.
- **Not decided:** Pi model and Mesa version; upstream PR.

## Facts vs assumptions

| Item | Status |
|---|---|
| Pre-merge fork DRM worked on a Raspberry Pi | Fact (user-reported). Exact commit assumed `339de50a`; to confirm. |
| `Window::new_headless_with_setup` is public and needs no display server | **Fact** (`window.rs` ~1126, `wgpu_canvas.rs` `open_headless` ~931, uses `Instance::new_without_display_handle()`). |
| `snap_begin`/`snap_finish` exist; one readback in flight; output is RGB (3 B/px), rows read bottom-to-top, then `flip_vertical` in `snap_finish` | **Fact** (`screenshot.rs`, `wgpu_canvas.rs` ~1582-1700). |
| Headless surface format is hard-coded `Rgba8Unorm` | **Fact** (`wgpu_canvas.rs` ~979). |
| Headless window sets `hidden: true`, `close_key: None`; renders to an offscreen target, never acquires a surface frame | **Fact** (`rendering.rs` ~215, 1349; `window.rs` do_new_headless). |
| Cameras read input via `canvas.get_key`/`get_mouse_button`; key state is updated only inside `WgpuCanvas`; no public event-injection API exists | **Fact** (grep). |
| Upstream requests only optional features plus `adapter.limits()`; clustered lighting/SSR fall back at runtime | **Fact.** Device creation should not fail on a Pi. |
| Upstream pipeline runs on Pi V3D (HDR `Rgba16Float` film, shadows, transmission) | **Unknown.** Needs Pi run. |
| No upstream code checks `max_color_attachments`; old DRM code notes V3D allows 4, not 8 | Fact (grep). Whether any pass needs more than 4: **unknown**. |
| Pi KMS planes accept `XBGR8888` (RGBA byte order) as well as `XRGB8888` | **Unknown.** Would remove a swizzle. Check on Pi. |
| wgpu 30 port of the old DRM code is mostly API fixes | Assumption. Old code was written against an older wgpu; unverified. |

## Pre-step: preserve the working state

- [x] Reference for the working DRM solution: `origin/master` at `339de50a`. Decided by user, 2026-10-27.
- [x] Merge fixes snapshotted on `sync` as `4705aa08` and pushed to `origin/sync`.
- [x] Local branches other than `sync` deleted. All remain on `origin` (`drm_support` 03e26d06, `drm_support_mk2` 36fc2537, `refactor` c5830d3f, `refactor2` 75e91564, `refactor3` aada7160, `traits` 09cab733, `master` 339de50a).
- [x] `git fetch upstream` run; `upstream/master` is `03727cd2` (v0.47.0).
- [x] Branch `drm` created off `upstream/master` (`03727cd2`); the first commit contains only this plan and `pi4_baseline/`.
- Reference documents from the old fork are **not** on this branch. They live on `origin/sync` (commit `4705aa08`): `git show origin/sync:DRM_ANALYSIS.md`, `EVDEV_IMPLEMENTATION_PLAN.md`, `GPU_READBACK_PIPELINE.md`, `context.md`, `plan.md` (the last two describe the abandoned merge and are obsolete).
- [ ] Optional: tag `339de50a` as `drm-pi-verified` (needs user approval; push is outward-facing).
- [ ] `traits` (`09cab733`) is unmerged and obsolete under this plan. Confirm and drop.
- [x] Pi tree compared against all fork branches: no exact commit match (closest `refactor2`). Baseline will use the prebuilt binaries on the Pi (see Measurements log).

---

# Part A: Spike results (re-run on v0.47.0)

**Method:** read-only `git show`/`git grep` on refs; no working-tree changes. Question 8 (Pi limits) not measured: no hardware access.

## Answers

| # | Question | Answer |
|---|---|---|
| 1 | Does `Canvas` need winit? | Not for headless. `Canvas::open_headless` and `WgpuCanvas::open_headless` build a surface-less wgpu context (`window: None`, `surface: None`). `Window::new_headless_with_setup(w, h, setup)` is the public entry point. |
| 2 | Can DRM reuse the offscreen path? | Yes, already. Headless windows have `hidden = true` and always render into `offscreen_output_target`; no surface acquire, no 2 s startup timeout. Offscreen is single-sampled. |
| 3 | Where does present/readback hook in? | **No hook needed for Tier 1.** After `render_*`, the frame is already copied into the canvas readback texture (`rendering.rs` ~1272, 1521). The DRM layer calls `snap_begin()` after frame N and `snap_finish()` after frame N+1, then hands pixels to the display thread. For Tier 3 only: a `present_offscreen` call beside those two `copy_texture_to_readback` lines. |
| 4 | wgpu features and limits? | Optional features only (`device_features(adapter, setup.required_features)`: timestamps, ray query when available); limits = `adapter.limits()`. Clustered lighting needs compute shaders, >=3 storage buffers per stage and `BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED`; checked at runtime. SSR is active only where clustered lighting is supported, so it turns itself off on weaker GPUs. |
| 5 | Which effects can be toggled? | Off by default: SSAO, SSR, DoF, bloom, auto-exposure, ray tracer. **On by default, toggleable:** shadows (`set_shadows_enabled`), transmission (`set_transmission_enabled`). Knob: `set_shadow_resolution` (default 2048 per layer). **Always on:** the HDR film (`Rgba16Float`) and tonemap pass. MSAA is not used offscreen. Cheapest valid config: shadows off, transmission off, others off. |
| 6 | Where does input plug in? | Cameras call `canvas.get_key`/`get_mouse_button`; the arrays live in `WgpuCanvas` and are written only by the winit handler. `out_events: Sender<WindowEvent>` is private to the canvas. evdev therefore needs a small `pub(crate)` or public injection method (update state arrays and send on `out_events`). `poll_events` currently returns early for window-less canvases. `EVDEV_IMPLEMENTATION_PLAN.md` phases 1-5 are reusable. |
| 7 | What in `drm/` carries over? | `card.rs` (86 lines) and `display_thread.rs` (420): independent of window design; wgpu-free in principle, verify. KMS helpers in `drm_canvas.rs` (`validate_device_capabilities`, `query_display_resources`, `find_available_crtc`, `select_best_mode`): carry over. `ReadbackPipeline` (~917-1100): only needed for Tier 2/3. **Do not port:** `drm_window.rs`, `window_common.rs`, `DrmSurfaceTexture`, `OffscreenBuffers`-based canvas, `init_wgpu_headless`, `DrmEventManager`, `CanvasInputState`. |
| 8 | Pi adapter limits? | **Not measured.** Only the old note: V3D `max_color_attachments = 4`. |

## Design options

| Tier | Description | Upstream files changed | Pi performance | Recommendation |
|---|---|---|---|---|
| **1. Consumer module (start here)** | `DrmWindow` owns a headless `Window` plus a display thread. Its `render_*` wrappers call the inner window, then `snap_begin`/`snap_finish`, then send the image to the display thread. `Deref` to `Window` for everything else. | `Cargo.toml`, `window/mod.rs` only | Lowest: upstream readback does RGB conversion, a bottom-up read, a `flip_vertical`, then DRM needs an RGB-to-XRGB expansion. One frame in flight. | Prototype first; measure |
| **2. Raw readback API** | Add a small generic API, e.g. `begin_read_pixels_raw`/`finish_read_pixels_raw` (native format, top-down, padding removed or visible via callback). Useful for any video-capture user, so plausible upstream. | + `wgpu_canvas.rs`, `canvas.rs`, `screenshot.rs`/`window.rs` (roughly 60-100 lines) | Near the old pipeline: memcpy of BGRA rows | Only if Tier 1 measures too slow |
| **3. Render-path hook** | v0.3 design: `present_offscreen` call at two sites in `rendering.rs` plus a DRM presenter in `WgpuCanvas`. | + `rendering.rs` | Same as old | Last resort; highest sync-conflict risk |

**Input (deferred, post-v1):** needs a small injection method on `Canvas`/`WgpuCanvas` (about 20-40 lines) plus an evdev reader module. This would be the one unavoidable upstream-file change. v1 has no input; the scene is driven programmatically, like `OffscreenSurface`.

**Pixel format:** headless is `Rgba8Unorm`. Tier 1 is unaffected (readback yields RGB). For Tier 2, prefer a DRM plane format matching RGBA byte order (`XBGR8888`) if the Pi's plane supports it; otherwise swizzle on CPU.

**Packaging:** the DRM layer can live as a module in this fork (`src/window/drm/`, feature `drm`) or as a separate crate (`kiss3d-drm`) depending on `kiss3d`. A separate crate removes the fork entirely for Tier 1/2 but cannot reach canvas internals for evdev until upstream exposes an injection API. Decision below.

## Go/no-go

**GO.** The "Go" criterion is met for Tier 1: no change to `rendering.rs`, `Window` or `WgpuCanvas`.

## Spike follow-ups that need hardware or Linux

- [x] Pi Vulkan limits dumped with `vulkaninfo` (2026-10-28), see Measurements log. Still to do: adapter info as wgpu sees it, logged by the old code and the new build.
- [ ] Confirm every upstream render pass stays within the Pi's color-attachment limit (V3D: 4). Upstream's non-clustered fallback path targets WebGL2-class limits, which suggests it fits, but this is an inference, not verified.
- [x] `Rgba16Float` and `R16Float` on V3D: color attachment, blending, linear filtering and storage image all supported (Vulkan format properties, optimal tiling). Runtime behavior of the HDR/OIT passes still to verify on the Pi.
- [ ] Confirm upstream v0.47.0 builds and links on Linux aarch64 and runs headless with no display server.
- [ ] Check which scanout formats the Pi's primary plane supports (`XRGB8888`, `XBGR8888`).

---

# Part B: Full plan (revised; sizes are relative: S small, M medium, L large)

## Target outcome (acceptance criteria)

- [ ] Branch is based on `upstream/master` (v0.47.0 or newer); Tier 1 diff against upstream touches only `Cargo.toml`, `src/window/mod.rs` and new `src/window/drm/` files (or a separate crate).
- [ ] Without `drm`: `cargo check --all-targets` and `cargo test --lib` pass as on upstream.
- [ ] With `drm` on Linux: `cargo check --all-targets --features drm` passes.
- [ ] `rendering.rs`, `window.rs` and `wgpu_canvas.rs` are unchanged unless a measured need justifies Tier 2/3 or the input injection point.
- [ ] On the Raspberry Pi: the new build renders the benchmark scene (125 cubes, one point light, 1024x600) to the display via DRM/KMS at an average of **at least 30 FPS** (target set by user 2026-10-28), measured with the light profile (shadows and transmission off). Also report FPS with default settings. The old pipeline measured 26.3 FPS, so the target is **above** the baseline and requires the 38 ms frame-time bottleneck to be diagnosed (Phase 6).

## Out of scope (for now)

- Windows/macOS DRM (DRM/KMS is Linux only).
- Making every upstream effect run well on a Pi; only a documented working configuration.
- Changing upstream's public API beyond additive DRM-related entry points.
- evdev input (keyboard, mouse, touch) and hotplug: deferred until after v1 (see Phase 8).

## Phases

| Phase | Work | Size | Verification | Depends on |
|---|---|---|---|---|
| 0 | Reference chosen; merge fixes pushed; upstream fetched; optional tag | S | `git log`, `git tag -l` | done except optional tag |
| 1 | Spike (re-run on v0.47.0) | done | Part A | |
| 2 | Baseline on Pi with `339de50a`: adapter info/limits, frame rate on a fixed scene and resolution | S (needs Pi) | numbers in Measurements log | 0 |
| 3 | Branch `drm` off `upstream/master`; add `drm` feature and dependency; cfg-gated `mod drm` | S | `cargo check` unchanged without feature | 1 |
| 4 | Port `card.rs`, `display_thread.rs`, KMS helpers; replace the old canvas types; fix for wgpu 30 where touched | M | `cargo check --features drm` on Linux | 3 |
| 5 | **Done.** Tier 1: `DrmWindow` (headless `Window` + `snap_begin`/`snap_finish` + display thread); offscreen-to-file mode for tests | M | example renders to the display on a Linux box; `snap_image` matches | 4 |
| 6 | Pi bring-up and measurement vs baseline; choose light-effects profile; decide Tier 2 if frame rate is worse than baseline | M (needs Pi) | numbers recorded | 5, 2 |
| 7 | Tier 2 (only if needed): raw readback API; consider upstream PR | M | frame rate within target | 6 |
| 8 | **Post-v1:** input: injection method on canvas plus evdev reader | M | cameras respond on Pi | 9 |
| 9 | Docs; CI for `--features drm` compile on Linux; rebase dry run on `upstream/master`; decide upstream PR. **Ends v1.** | S | `git rebase upstream/master` succeeds | 6 |

## Progress log

- **Phase 3 done (2026-10-28):** `drm` feature plus Linux-only optional `drm = "0.14.1"` dependency in `Cargo.toml`; `#[cfg(all(feature = "drm", target_os = "linux"))] mod drm;` in `src/window/mod.rs`. Only those two upstream files are touched.
- **Phase 4 done (2026-10-28):** ported to `src/window/drm/`:
  - `card.rs` and `display_thread.rs`: verbatim from `origin/master` (`339de50a`); no wgpu dependency, nothing needed fixing for wgpu 30.
  - `kms.rs`: connector, CRTC and mode discovery plus capability check, extracted from the old `DrmCanvas` into free functions (behavior unchanged; `select_best_mode` now uses `.first().copied().ok_or_else(..)` instead of `unwrap`).
  - `error.rs`: `DrmError` (renamed from `DrmCanvasError`, wgpu-specific variants dropped).
  - `mod.rs`: `#![allow(dead_code)]` until the consumer (`DrmWindow`, phase 5) exists; remove it then.
- **Verification (Mac, cross-compiled):** `cargo check --lib --tests --target aarch64-unknown-linux-gnu --features drm` passes, no warnings; `cargo clippy` shows nothing for `drm/`; `cargo fmt --check` clean; host `cargo check --lib` (no feature) passes. **Not run:** the unit tests (they compile but were not executed; `BufferPool` and `Card` tests need a Linux host) and anything on the Pi.
- **Build note:** `Cargo.lock` is git-ignored, so a fresh resolution picked `glamx 0.3.0` with `glam 0.33.1`, which lacks the `glam::camera` module upstream uses. `cargo update -p glamx` (to `glamx 0.3.1`, `glam 0.33.12`) fixes it. Any other checkout, including the Pi, needs the same step, or the ignored lockfile must be copied along. Worth reporting upstream.
- **For phase 5:** `DisplayCommand.pixel_data` expects XRGB8888 (B, G, R, X in memory), whereas `Window::snap_finish` yields RGB (3 bytes per pixel, top-left origin). Do the RGB to XRGB expansion on the display worker thread, not on the render thread, e.g. by adding a pixel-format field to `DisplayCommand`.

- **Phase 5 done (2026-10-28):** `DrmWindow` (`src/window/drm/window.rs`, re-exported as `kiss3d::window::DrmWindow` and `DrmError`) wraps `Window::new_headless_with_setup` (single-sampled) and presents with `snap_finish` then `snap_begin` after every `render*` call, one frame behind. `DisplayThread` now takes RGB frames, converts to XRGB8888 per row in cached memory on the worker thread, honors the dumb-buffer pitch; the old `BufferPool` and recycler were removed (unused). New `examples/drm_benchmark.rs` (feature `drm`, same scene as the old benchmark, real frame-time statistics, `--light`, `--frames`). Wired into `Cargo.toml` (`[[example]] required-features`) and `src/window/mod.rs`.
- **Phase 5 verification:** Mac cross-check (aarch64, lib, tests, examples), clippy and fmt clean. **On the Pi:** release build of `drm_benchmark` succeeded (38 min, `CARGO_BUILD_JOBS=2`, 0 warnings), runs on the V3D (log line `Rendering on "V3D 4.2.14.0" (IntegratedGpu, Vulkan)`), no errors, `get_throttled=0x0` before and after. Unit tests (RGB to XRGB row conversion) were not run on the Pi. **Visual check passed (user, 2026-10-28):** a longer light-profile run (1,860 frames, 61.4 s, 30.5 FPS avg, 35.0 ms p95, no errors, no throttling; log `pi4_baseline/drm_benchmark_new_light_visual_2026-10-28.log`) showed the correct picture on the HDMI display.
- **Pi results, new build, 1024x600, 125 cubes, 360 frames (60 warm-up), logs in `pi4_baseline/`:**

| Profile | Avg FPS | Frame time ms (avg / min / p50 / p95 / p99 / max) | Result vs targets |
|---|---|---|---|
| light (shadows and transmission off) | **30.2** | 33.12 / 28.03 / 33.32 / 34.26 / 36.70 / 40.26 | Meets the 30 FPS target, with almost no margin (p95 is 29.2 FPS) |
| default | **11.6** | 86.57 / 83.89 / 86.59 / 88.30 / 89.78 / 90.80 | Shadows and/or transmission cost about 53 ms per frame |
| old pipeline (baseline) | 26.3 | 38.09 / 37.23 / 38.11 / 38.43 / 38.62 / 38.68 | reference |

  The CPU was warm at the start of each run (about 69 to 70 C after the build), no throttling.
- **FPS target rule:** target stays 30 FPS for the light profile. If profiling later shows the hardware cannot reach it, fall back to the old baseline of 26.3 FPS (user decision 2026-10-28). Not needed so far.

## Risks and dependencies

| Risk | Impact | Mitigation |
|---|---|---|
| 30 FPS target is above the 26.3 FPS baseline, and the old frame time is flat at 38 ms (bottleneck undiagnosed: GPU fill, readback, or display thread) | Target missed even if the port is correct | Phase 6: profile GPU time (timestamp queries), readback and display-thread time separately; Tier 2 raw readback; lower render resolution or effects |
| Upstream readback CPU cost (RGB conversion, double row reversal, RGB-to-XRGB expansion, one frame in flight) on a Pi | Lower frame rate than the old pipeline | Measure in Phase 6; Tier 2 raw readback if needed |
| Upstream HDR pipeline (`Rgba16Float`, OIT, shadows, transmission) exceeds V3D capability or speed | DRM slow or unusable on Pi | Phase 2 baseline; light profile via runtime toggles; HDR film cannot be disabled, so a failure there needs an upstream-facing change |
| A render pass needs more than 4 color attachments | Pipeline creation error on Pi | Follow-up check; fix per pass |
| evdev (post-v1) needs canvas internals | Forces an upstream-file change or an upstream PR | Deferred; keep the injection method tiny and generic when done |
| Plane does not support the byte order chosen | Wrong colors | Query planes; use `XRGB8888`/`XBGR8888` accordingly; verify visually |
| wgpu 30 differences in any retained wgpu code | Port takes longer | Tier 1 keeps almost no wgpu code in `drm/` |
| Cannot test DRM on macOS | Late discovery of runtime bugs | Linux CI for compile; Pi for runtime; baseline from `339de50a` |
| Upstream churn (v0.44 to v0.47 in 4 months) | Sync conflicts | Tier 1 touches no hot files; rebase regularly |

## Decisions pending

| Decision | Recommendation | Owner |
|---|---|---|
| Packaging: module in this fork vs. separate crate `kiss3d-drm` | Module in the fork is fine for v1. With evdev deferred, Tier 1 uses only public API, so a separate crate `kiss3d-drm` (no fork) is now viable; revisit before Phase 3 | User |
| Start with Tier 1 and escalate only on measured need | Yes | User |
| Offer a raw-readback API and/or input injection upstream | Yes if Tier 2/evdev are needed; both are generic and small | User |
| evdev input in v1 scope? | **Decided: no.** Post-v1 (Phase 8). | User |
| Minimum Pi model and OS/driver (Mesa version) | **Model: Raspberry Pi 4 available (2026-10-27).** OS, Mesa version and how to access it (ssh, build on device vs. cross-compile) still to record before Phase 2 | User |
| Branch name | `drm` in this repo off `upstream/master`; old work stays reachable via `origin/sync`, `origin/master` and the optional tag | User |

## Phase 2 procedure: Pi 4 baseline with `339de50a`

Goal: know what "working" looks like before changing anything.

1. Record: OS and kernel (`uname -a`, `/etc/os-release`), Mesa and Vulkan driver versions (`vulkaninfo --summary`), display connector and mode, `config.txt` GPU settings (`dtoverlay=vc4-kms-v3d`), RAM/GPU memory split.
2. Check out `339de50a` on the Pi, build with `--release --features drm`, run with `RUST_LOG=debug` to capture `adapter.get_info()` and `adapter.limits()` (the old code logs both).
3. Fix one scene and one resolution (the display's native mode, plus one smaller mode such as 720p). Record average and worst-case frame time over at least 60 s, and CPU use of the main and display threads.
4. Save the log and a screenshot under this plan's measurements log (or a linked file).
5. Repeat step 3 later with the new build, same scene, same resolution.

## Pi dev cycle

```
# on the Mac: dry run first (-n), then the real sync. Host name wodio.fritz.box did not
# resolve from the dev Mac on 2026-10-28; the IP did.
rsync -avPn /Users/flimburg/OpenSource/kiss3d_fl/ flimburg@192.168.5.64:/home/flimburg/<DEST>/ \
  --exclude target --exclude .git --exclude .DS_Store --exclude .ssh --exclude .cargo
rsync -avP  /Users/flimburg/OpenSource/kiss3d_fl/ flimburg@192.168.5.64:/home/flimburg/<DEST>/ \
  --exclude target --exclude .git --exclude .DS_Store --exclude .ssh --exclude .cargo
# on the Pi (release only, limited jobs because of 1.8 GB RAM)
export PATH="$HOME/.cargo/bin:$PATH"; cd ~/<DEST>
CARGO_BUILD_JOBS=2 cargo build --release --features drm --example <name>
```

- **Flags:** these are the flags from the previous dev cycle. No `--delete`, so files removed locally stay on the Pi; stale files are harmless as long as `mod.rs` no longer references them.
- **Do not sync into `~/kiss3d_fl` yet.** A dry run (2026-10-28, from the `sync` branch tree) showed 314 files to send: 164 new and **150 overwritten**, including `Cargo.toml`, `Cargo.lock`, all of `src/window/drm/*` and several `examples/drm_*.rs`. The Pi's tree is not under git and matches no commit, so those files are the only record of what the baseline binaries were built from.
- **Before any sync into `~/kiss3d_fl`:** save a tarball of `src examples Cargo.toml Cargo.lock` (a few MB) on the Pi, and copy the baseline binaries from `target/release/examples/` to a separate directory, because a rebuild replaces them. `<DEST>` for the new branch should be `kiss3d_drm`, not `kiss3d_fl`, once the baseline is secured.

- **Disk:** 6.6 GB free. `~/kiss3d_fl/target/debug` is 8.9 GB and `target/release` (which holds the baseline binaries) is 1.2 GB. A new release-only build of wgpu 30 may need several GB. **Proposal (needs approval):** delete only `~/kiss3d_fl/target/debug`; do not build debug on the Pi.
- **Memory:** 1.8 GB RAM plus 2 GB swap; keep `CARGO_BUILD_JOBS=2` and watch for OOM during the large link steps.
- **MSRV:** upstream declares no `rust-version`. wgpu 29 needs 1.87; wgpu 30's requirement is unverified. The Pi has 1.92, which is likely but not confirmed to be enough; the first build will show.
- **Before each DRM run:** make sure the `wodio` autostart is not running (`pgrep -a wodio`); stop it with SIGINT if needed. Leave `wodio-pi-controls` alone.
- **Console:** no X/Wayland session (boot target `multi-user.target`); the DRM path needs that.

## Measurements log (fill in)

### Baseline run: prebuilt `drm_async_benchmark` (2026-10-28)

Binary: `~/kiss3d_fl/target/release/examples/drm_async_benchmark` (built 2026-02-25, old fork code, unversioned source). Run: `DRM_DEVICE=/dev/dri/card1 RUST_LOG=kiss3d=debug,info`, 360 frames (60 warmup, 300 measured), 125 cubes, one point light, no `wodio` running, `get_throttled=0x0` before and after, temp 56.0 to 58.4 C, governor `ondemand`. Exit code 0.

| Metric | Result |
|---|---|
| Adapter | V3D 4.2.14.0 (confirmed by the log, not llvmpipe) |
| Requested size / actual scanout | 1920x1080 requested; canvas used the display mode **1024x600 @ 60 Hz** (buffer pool 3 x 2,457,600 B) |
| Average FPS | **26.3** (total 21.50 s for 360 frames) |
| Frame time avg / min / max / P95 / P99 | 38.09 / 37.23 / 38.68 / 38.43 / 38.62 ms |
| Surface format | `Bgra8Unorm`, DRM fourcc XRGB8888 (as in the old `choose_formats`) |
| Warnings | `VK_EXT_physical_device_drm` not found; missing downlevel flag `FULL_DRAW_INDEX_UINT32`; compute shaders reported available |

Caveats:
- The example's "Present() 0.000 ms" figures are meaningless: it measures an empty interval (`present_start.elapsed()` right after taking the timestamp). Only the frame time (all of it inside `render_3d`) is real. Do not use the "EXCELLENT" analysis text.
- Frame time is flat (37 to 39 ms) at about 26 FPS, which suggests a fixed bottleneck (GPU fill, readback, or the display thread's backpressure), not scene variance. Not yet diagnosed.
- The old renderer is a simple forward pipeline. Upstream v0.47.0 always renders through an HDR film plus tonemap pass, so equal-or-better FPS than this baseline is not a realistic target. See the revised acceptance criterion in Part B.
- Files: `pi4_baseline/baseline_drm_async_benchmark_2026-10-28.log`, `pi4_baseline/vulkaninfo_*_2026-10-28.txt` (summary, full, formats).

### Pi 4 Vulkan survey (`vulkaninfo`, 2026-10-28)

Two Vulkan devices are visible: GPU0 **V3D 4.2.14.0** (integrated, `V3DV Mesa` 24.2.8, Vulkan API 1.2.289) and GPU1 **llvmpipe** (CPU software rasterizer). Vulkan instance 1.3.239. Full dumps are on the Pi in `/tmp/vkinfo_pi4.txt` and `/tmp/vkfmt_pi4.txt` (volatile; copy them if needed).

| V3D limit | Value | Relevance |
|---|---|---|
| `maxColorAttachments` | **4** | Upstream passes must not exceed 4 (unverified) |
| `maxPerStageDescriptorStorageBuffers` | **8** | Clustered lighting needs at least 3; satisfied |
| `maxPerStageDescriptorSampledImages` / `Samplers` | 16 / 24 | Material pipelines with shadow atlas, IBL, probes, SSAO, transmission must fit in 16 sampled textures per stage |
| `maxPerStageDescriptorUniformBuffers` | 16 | |
| `maxImageDimension2D` / `maxFramebufferWidth` | 4096 / 4096 | Shadow atlas 2048 per layer is fine |
| `maxImageArrayLayers` | 2048 | Atlas has up to 16 layers |
| `maxBoundDescriptorSets` | 16 | |
| Compute | 256 invocations, 16 KiB shared memory | Clustered light culling should be able to run |
| Device-local heap | **923 MiB** (system RAM 1.8 GiB shared) | Shadow atlas worst case: `Depth32Float` 2048 x 2048 x 16 layers = 256 MiB (allocated per light count, up to 16 layers). Lower `set_shadow_resolution` or disable shadows on the Pi |
| Formats | `R16G16B16A16_SFLOAT`, `R16_SFLOAT`: color attachment, blend, linear filter, storage | HDR film and OIT formats are usable |
| Features | `independentBlend`, `fragmentStoresAndAtomics`, `imageCubeArray`, `sampleRateShading`, `shaderStorageImageExtendedFormats`: true; `dualSrcBlend`: false; `depthClamp`: false | Upstream use of `depthClamp`/dual-source blending is unchecked |

**Adapter selection risk:** with llvmpipe present, a wrong adapter choice silently renders in software. Log `adapter.get_info()` at startup and assert the backend is V3D on the Pi. The old code used `PowerPreference::HighPerformance`; upstream headless uses the default preference.

### Pi 4 facts (read-only survey, 2026-10-28, host `wodio`, 192.168.5.64, key login as `flimburg`)

| Item | Value |
|---|---|
| Model / RAM | Raspberry Pi 4 Model B Rev 1.1, 1.8 GB usable (about 2 GB model), 2 GB swap |
| OS / kernel | Debian 12 (bookworm) aarch64, kernel 6.12.25+rpt-rpi-v8 |
| Mesa | `mesa-vulkan-drivers` and `libgl1-mesa-dri` 24.2.8-1~bpo12+rpt4 |
| Vulkan tools | `vulkaninfo` not installed (needs `vulkan-tools` for the limits dump) |
| Display stack | **`dtoverlay=vc4-fkms-v3d`** (firmware KMS, not full `vc4-kms-v3d`); boots to console (`multi-user.target`, no display manager) |
| DRM nodes | `/dev/dri/card0`, `card1`, `renderD128`; the connected output is **`card1-HDMI-A-1`** with a forced custom mode 1024x600@60 (`hdmi_cvt`). The old example defaults to `card0` unless `DRM_DEVICE` is set |
| CPU state | governor `ondemand` (not `performance`), `throttled=0x0` at survey time |
| Disk | 59 GB card, 6.6 GB free (89% used); `~/kiss3d_fl` alone is 10 GB |
| Toolchain | `~/.cargo/bin` has cargo and clippy (not on the non-interactive PATH; version not checked); git, gcc, pkg-config present |
| Other software | systemd service `wodio-pi-controls` (running, left untouched); `wodio` autostarted from `~/.profile` line 31 (`cd wodio && ./target/release/wodio -g -A 800 -R 8080 -i`) |
| Existing kiss3d tree | `~/kiss3d_fl`: not a git repo; closest to `refactor2` (152 of 181 files under `src/` and `examples/` identical) plus extra unversioned DRM files; no commit matches exactly. Prebuilt release examples dated Feb 2026: `drm_async_benchmark`, `drm_cube`, `drm_cubes`, `drm_display_test`, `drm_offscreen_cube`, `drm_test` |

Implications:
- `339de50a` cannot be confirmed as the Pi-verified commit by file match. The prebuilt binaries on the Pi are the actual known-good artifact; use them for the baseline, and rebuild `339de50a` only as a second reference.
- `fkms` vs full KMS may matter for Vulkan (v3dv) and DRM behavior; keep the current config for the baseline and note it with every result. Do not change `config.txt` without approval.
- Dev cycle (as before): rsync the code from the Mac to the Pi, build and run on the Pi. Toolchain there: cargo/rustc 1.92.0 stable, rsync, git, gcc, `libgbm-dev`, 4 cores. See "Pi dev cycle" above for disk and memory limits.
- The display is 1024x600; the old benchmark requests 1920x1080. Check what the old code actually scanned out.
- Every login on tty1 restarts the `wodio` autostart through `~/.profile`; stop it before any DRM test (it holds the display). Interrupted with SIGINT on 2026-10-28.

_Phase 2 and Phase 6 results go here: Pi model, Mesa version, adapter info, limits, scene, resolution, frame rate._
