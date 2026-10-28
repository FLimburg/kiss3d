//! DRM/KMS display output for headless windows (Linux only).
//!
//! A headless [`Window`](crate::window::Window) renders off-screen; this module
//! shows its frames on a display without a window system, by writing them into
//! dumb buffers on a separate thread. See `DRM_REBASE_PLAN.md` for the design.

// The consumers (`DrmWindow`, plan phase 5) are not written yet.
#![allow(dead_code)]

mod card;
mod display_thread;
mod error;
mod kms;

#[allow(unused_imports)]
pub use error::DrmError;
