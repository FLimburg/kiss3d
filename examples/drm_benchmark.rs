//! Renders a fixed scene to a display through DRM/KMS and reports frame times.
//!
//! Needs a Linux console without a window system; run it with the DRM device that
//! has the connected display (`DRM_DEVICE`, default `/dev/dri/card0`):
//!
//! ```bash
//! DRM_DEVICE=/dev/dri/card1 cargo run --release --features drm --example drm_benchmark
//! ```
//!
//! Options:
//! - `--no-shadows`, `--no-transmission`: turn that effect off; `--light` turns both off.
//! - `--tonemap <none|reinhard|aces|agx|neutral|tony>`: tonemapping operator (default: kiss3d's).
//! - `--split`: time rendering and presenting separately, and print per-pass GPU times
//!   when the device supports timestamp queries.
//! - `--frames <n>`: number of measured frames (default 300, after 60 warm-up frames).
//!
//! The scene (125 small cubes, one point light, orbit camera) is the one the former
//! DRM backend's `drm_async_benchmark` used, so the numbers are comparable.

#[cfg(target_os = "linux")]
mod benchmark {
    use kiss3d::post_processing::Tonemap;
    use kiss3d::prelude::*;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const WARMUP_FRAMES: usize = 60;

    fn percentile(sorted: &[Duration], p: f64) -> Duration {
        let index = ((sorted.len() - 1) as f64 * p).round() as usize;
        sorted[index]
    }

    pub async fn run() {
        env_logger::init();

        let args: Vec<String> = std::env::args().collect();
        let has = |flag: &str| args.iter().any(|a| a == flag);
        let light = has("--light");
        let shadows = !(light || has("--no-shadows"));
        let transmission = !(light || has("--no-transmission"));
        let split = has("--split");
        let value_of = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let tonemap = value_of("--tonemap").and_then(|name| match name.as_str() {
            "none" => Some(Tonemap::None),
            "reinhard" => Some(Tonemap::Reinhard),
            "aces" => Some(Tonemap::Aces),
            "agx" => Some(Tonemap::AgX),
            "neutral" => Some(Tonemap::Neutral),
            "tony" => Some(Tonemap::TonyMcMapface),
            _ => {
                eprintln!("Unknown tonemap '{}', using the default", name);
                None
            }
        });
        let frames = value_of("--frames")
            .and_then(|n| n.parse().ok())
            .unwrap_or(300usize);

        let device_path =
            std::env::var("DRM_DEVICE").unwrap_or_else(|_| "/dev/dri/card0".to_string());
        let mut window = match DrmWindow::new(&device_path).await {
            Ok(window) => window,
            Err(e) => {
                eprintln!("Failed to open {}: {}", device_path, e);
                return;
            }
        };
        window.set_shadows_enabled(shadows);
        window.set_transmission_enabled(transmission);
        if let Some(tonemap) = tonemap {
            window.set_tonemap(tonemap);
        }

        let mut camera = OrbitCamera3d::default();
        camera.set_yaw(std::f32::consts::PI / 4.0);
        camera.set_pitch(-std::f32::consts::PI / 6.0);
        camera.set_dist(8.0);

        let mut scene = SceneNode3d::empty();
        scene
            .add_light(Light::point(100.0))
            .set_position(Vec3::new(5.0, 5.0, 5.0));

        let grid_size = 5;
        let spacing = 2.0;
        let mut cubes = Vec::new();
        for x in 0..grid_size {
            for y in 0..grid_size {
                for z in 0..grid_size {
                    let center = grid_size as f32 / 2.0;
                    let mut cube = scene.add_cube(0.4, 0.4, 0.4);
                    cube.set_position(Vec3::new(
                        (x as f32 - center) * spacing,
                        (y as f32 - center) * spacing,
                        (z as f32 - center) * spacing,
                    ));
                    cube.set_color(Color::new(
                        x as f32 / grid_size as f32,
                        y as f32 / grid_size as f32,
                        z as f32 / grid_size as f32,
                        1.0,
                    ));
                    cubes.push(cube);
                }
            }
        }

        println!(
            "{}x{}, {} cubes, shadows: {}, transmission: {}, tonemap: {:?}, {} warm-up + {} measured frames",
            window.width(),
            window.height(),
            cubes.len(),
            shadows,
            transmission,
            window.hdr_settings().tonemap,
            WARMUP_FRAMES,
            frames
        );

        println!(
            "Clustered lighting supported: {}",
            Context::get().supports_clustered_lighting()
        );

        let rot = Quat::from_axis_angle(Vec3::new(0.3, 0.7, 0.2).normalize(), 0.01);
        let mut frame_times = Vec::with_capacity(frames);
        let mut render_times = Vec::with_capacity(frames);
        let mut present_times = Vec::with_capacity(frames);
        let mut submit_times = Vec::with_capacity(frames);
        let mut gpu_times = Vec::with_capacity(frames);
        let mut gpu_passes: HashMap<&'static str, (Duration, u32)> = HashMap::new();
        let start = Instant::now();

        for frame in 0..WARMUP_FRAMES + frames {
            let frame_start = Instant::now();
            for cube in &mut cubes {
                cube.rotate(rot);
            }
            let running = if split {
                // `Window::render_3d` renders without presenting; present separately.
                let render_start = Instant::now();
                let running = Window::render_3d(&mut window, &mut scene, &mut camera).await;
                let present_start = Instant::now();
                window.present();
                if frame >= WARMUP_FRAMES {
                    render_times.push(present_start - render_start);
                    present_times.push(present_start.elapsed());
                }
                running
            } else {
                window.render_3d(&mut scene, &mut camera).await
            };
            if !running {
                break;
            }
            if frame >= WARMUP_FRAMES {
                frame_times.push(frame_start.elapsed());
                if let Some(timings) = window.render_timings() {
                    submit_times.push(timings.cpu_submit);
                    if let Some(steps) = &timings.gpu_steps {
                        gpu_times.push(timings.gpu_total().unwrap_or_default());
                        for (name, duration) in steps {
                            let entry = gpu_passes.entry(*name).or_default();
                            entry.0 += *duration;
                            entry.1 += 1;
                        }
                    }
                }
            }
        }

        if frame_times.is_empty() {
            eprintln!("No frame was measured");
            return;
        }

        let total = start.elapsed();
        let mean = frame_times.iter().sum::<Duration>() / frame_times.len() as u32;
        frame_times.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        println!("Total time: {:.2}s", total.as_secs_f64());
        println!(
            "Average FPS (measured frames): {:.1}",
            1.0 / mean.as_secs_f64()
        );
        println!(
            "Frame time ms: avg {:.2}, min {:.2}, p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.2}",
            ms(mean),
            ms(frame_times[0]),
            ms(percentile(&frame_times, 0.50)),
            ms(percentile(&frame_times, 0.95)),
            ms(percentile(&frame_times, 0.99)),
            ms(*frame_times.last().unwrap()),
        );

        if let Some(timings) = window.render_timings() {
            println!("Last frame: {}", timings);
        }

        let average = |times: &[Duration]| {
            (!times.is_empty()).then(|| ms(times.iter().sum::<Duration>() / times.len() as u32))
        };
        if split {
            println!(
                "Split ms (avg): render {:.2}, present {:.2}",
                average(&render_times).unwrap_or_default(),
                average(&present_times).unwrap_or_default()
            );
        }
        if let Some(submit) = average(&submit_times) {
            println!("CPU submit ms (avg): {:.2}", submit);
        }
        match average(&gpu_times) {
            Some(gpu) => {
                println!("GPU pass time ms (avg, sum of timed passes): {:.2}", gpu);
                let mut passes: Vec<_> = gpu_passes.into_iter().collect();
                passes.sort_by_key(|(_, (total, count))| std::cmp::Reverse(*total / *count));
                for (name, (total, count)) in passes.iter().take(8) {
                    println!("  {:<24} {:.2} ms", name, ms(*total / *count));
                }
            }
            None => println!("GPU pass times: not available on this device"),
        }
    }
}

#[cfg(target_os = "linux")]
#[kiss3d::main]
async fn main() {
    benchmark::run().await;
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("DRM/KMS is only available on Linux");
}
