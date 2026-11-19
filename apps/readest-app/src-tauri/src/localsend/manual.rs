//! Manual (host:port) connection for networks where multicast discovery is
//! blocked — cross-VLAN, AP isolation, or multicast-hostile Wi-Fi. Dials the
//! peer's `/info` endpoint once and feeds the answer into the discovery store,
//! so the device then behaves exactly like a discovered one (pairing, send,
//! presence probing all reuse the normal paths).

use crate::localsend::{events::device_type_str, LocalSendState};
use localsend::discovery::{DeviceChannel, DiscoveredDevice, DiscoveryHandle, HttpChannel};
use localsend::http::client::v2::LsHttpClientV2;
use localsend::model::discovery::ProtocolType;
use serde::Serialize;
use std::time::Duration;
use tauri::State;

/// What the frontend shows after a successful manual connect (a subset of the
/// peer's `/info` answer; the full device goes into the discovery store).
#[derive(Serialize)]
pub struct ManualPeerInfo {
    pub alias: String,
    pub device_model: Option<String>,
    pub device_type: Option<String>,
    pub fingerprint: String,
}

/// Probes `https://<host>:<port>/api/localsend/v2/info` and registers the
/// answering device. The first dial accepts any server certificate — this is
/// discovery, per the client crate's documented contract; from then on the
/// send path pins the peer fingerprint like for any discovered device.
pub async fn connect_manual(
    discovery: &DiscoveryHandle,
    key_pem: &str,
    cert_pem: &str,
    host: String,
    port: u16,
) -> Result<ManualPeerInfo, String> {
    let client = LsHttpClientV2::try_new(key_pem, cert_pem, None, Some(Duration::from_secs(5)))
        .map_err(|e| format!("client setup failed: {e}"))?;
    let info = client
        .info(ProtocolType::Https, &host, port)
        .await
        .map_err(|e| format!("no LocalSend service at {host}:{port}: {e}"))?;
    let device = DiscoveredDevice {
        alias: info.alias.clone(),
        version: info.version.clone(),
        device_model: info.device_model.clone(),
        device_type: info.device_type.clone(),
        fingerprint: info.fingerprint.clone(),
        channel: DeviceChannel::Http(HttpChannel {
            host,
            port,
            protocol: ProtocolType::Https,
        }),
        download: info.download,
    };
    discovery.add_device(device).await;
    Ok(ManualPeerInfo {
        alias: info.alias,
        device_model: info.device_model,
        device_type: device_type_str(&info.device_type),
        fingerprint: info.fingerprint,
    })
}

#[tauri::command]
pub async fn localsend_connect_manual(
    state: State<'_, LocalSendState>,
    host: String,
    port: u16,
) -> Result<ManualPeerInfo, String> {
    let host = host.trim().to_string();
    if host.is_empty() {
        return Err("host must not be empty".into());
    }
    let guard = state.0.lock().await;
    let Some(service) = guard.as_ref() else {
        return Err("LocalSend is not running".into());
    };
    connect_manual(
        &service.discovery,
        &service.identity.key_pem,
        &service.identity.cert_pem,
        host,
        port,
    )
    .await
}
