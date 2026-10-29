use kiss3d::prelude::*;
use kiss3d::window::DrmWindow;

// Shows a rotating cube on a display through DRM/KMS, with no window system (e.g. on a
// console-only Raspberry Pi). Run it from a text console, not from inside a desktop session.
// Set `DRM_DEVICE` to pick another device than `/dev/dri/card0`.
#[kiss3d::main]
async fn main() {
    env_logger::init();
    let device = std::env::var("DRM_DEVICE").unwrap_or_else(|_| "/dev/dri/card0".to_string());
    let mut window = DrmWindow::new(&device)
        .await
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", device, e));
    // Shadow mapping dominates the frame time on small GPUs such as the Raspberry Pi 4.
    window.set_shadows_enabled(false);

    let mut camera = OrbitCamera3d::default();
    let mut scene = SceneNode3d::empty();
    scene
        .add_light(Light::point(100.0))
        .set_position(Vec3::new(0.0, 2.0, -2.0));

    let mut c = scene.add_cube(1.0, 1.0, 1.0).set_color(RED);
    let rot = Quat::from_axis_angle(Vec3::Y, 0.014);

    while window.render_3d(&mut scene, &mut camera).await {
        c.rotate(rot);
    }
}
