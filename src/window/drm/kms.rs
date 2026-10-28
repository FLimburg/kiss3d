//! KMS discovery: find a connected display, a CRTC that can drive it, and a mode.
//!
//! Ported from the former `DrmCanvas` helpers; they never depended on wgpu.

use super::card::Card;
use super::error::DrmError;
use drm::control::{connector, crtc, Device as ControlDevice, Mode, ResourceHandles};
use drm::Device;

/// Display configuration discovered from the hardware.
pub struct DisplayConfig {
    pub connector: connector::Handle,
    pub crtc: crtc::Handle,
    pub mode: Mode,
    pub width: u32,
    pub height: u32,
}

/// Checks that the device supports the capabilities the display path uses.
///
/// Fails early with a clear message instead of a cryptic error later.
pub fn validate_device_capabilities(card: &Card) -> Result<(), DrmError> {
    use drm::DriverCapability;

    log::debug!("Validating DRM device capabilities");

    match card.get_driver_capability(DriverCapability::DumbBuffer) {
        Ok(value) if value > 0 => {
            log::debug!("DumbBuffer capability: supported");
        }
        Ok(_) => {
            return Err(DrmError::Init(
                "Device does not support dumb buffers (required for display output)".to_string(),
            ));
        }
        Err(e) => {
            // Some drivers do not report capabilities properly: do not fail.
            log::warn!(
                "Could not query DumbBuffer capability: {}. Proceeding anyway.",
                e
            );
        }
    }

    Ok(())
}

/// Queries the display resources and picks a connector, CRTC and mode.
pub fn query_display_resources(card: &Card) -> Result<DisplayConfig, DrmError> {
    log::info!("Querying DRM display resources");

    let resources = card
        .resource_handles()
        .map_err(|e| DrmError::Drm(format!("Failed to get resource handles: {}", e)))?;

    let connector_info = find_connected_connector(card, &resources)?;
    log::info!(
        "Found connected connector: {:?} (id: {:?})",
        connector_info.interface(),
        connector_info.handle()
    );

    let crtc = find_available_crtc(card, &connector_info, &resources)?;
    log::info!("Selected CRTC: {:?}", crtc);

    let mode = select_best_mode(&connector_info)?;
    let (width, height) = mode.size();
    log::info!(
        "Selected mode: {}x{} @ {}Hz",
        width,
        height,
        mode.vrefresh()
    );

    Ok(DisplayConfig {
        connector: connector_info.handle(),
        crtc,
        mode,
        width: width as u32,
        height: height as u32,
    })
}

/// Finds the first connected connector.
fn find_connected_connector(
    card: &Card,
    resources: &ResourceHandles,
) -> Result<connector::Info, DrmError> {
    for &conn_handle in resources.connectors() {
        let conn_info = card
            .get_connector(conn_handle, false)
            .map_err(|e| DrmError::Drm(format!("Failed to get connector info: {}", e)))?;

        if conn_info.state() == connector::State::Connected {
            return Ok(conn_info);
        }
    }

    Err(DrmError::NoConnectedDisplay)
}

/// Finds a CRTC that can drive the given connector.
///
/// Walks the connector's encoders and checks each encoder's `possible_crtcs`
/// bitmask. This matters on embedded systems (Raspberry Pi, ...) where the CRTC
/// routing is fixed.
fn find_available_crtc(
    card: &Card,
    connector_info: &connector::Info,
    resources: &ResourceHandles,
) -> Result<crtc::Handle, DrmError> {
    log::debug!(
        "Finding compatible CRTC for connector {:?}",
        connector_info.handle()
    );

    let all_crtcs = resources.crtcs();
    if all_crtcs.is_empty() {
        return Err(DrmError::Modeset(
            "No CRTCs available in system".to_string(),
        ));
    }

    log::debug!("System has {} CRTCs available", all_crtcs.len());

    for &encoder_handle in connector_info.encoders() {
        log::debug!("Checking encoder {:?}", encoder_handle);

        let encoder = match card.get_encoder(encoder_handle) {
            Ok(enc) => enc,
            Err(e) => {
                log::warn!("Failed to get encoder {:?}: {}", encoder_handle, e);
                continue;
            }
        };

        let compatible_crtcs = resources.filter_crtcs(encoder.possible_crtcs());

        log::debug!(
            "Encoder {:?} has {} compatible CRTCs",
            encoder_handle,
            compatible_crtcs.len()
        );

        if let Some(&crtc_handle) = compatible_crtcs.first() {
            log::info!(
                "Selected CRTC {:?} for connector {:?} via encoder {:?}",
                crtc_handle,
                connector_info.handle(),
                encoder_handle
            );
            return Ok(crtc_handle);
        }
    }

    Err(DrmError::Modeset(format!(
        "No compatible CRTC found for connector {:?}. Connector has {} encoders.",
        connector_info.handle(),
        connector_info.encoders().len()
    )))
}

/// Selects the display mode: the first one, which is usually the preferred/native mode.
fn select_best_mode(connector_info: &connector::Info) -> Result<Mode, DrmError> {
    connector_info
        .modes()
        .first()
        .copied()
        .ok_or_else(|| DrmError::Modeset("No display modes available".to_string()))
}
