// Bluetooth through BlueZ's D-Bus API: the adapter, paired devices
// (speakers for audio out, and phones or computers using the instrument's
// Bluetooth MIDI), scanning and pairing. haxo001 registers the pairing
// agent when its Bluetooth MIDI is on.
use std::collections::HashMap;
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use log::info;
use serde::{Deserialize, Serialize};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::error::{Error, Result};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "/org/bluez/hci0";
const SCAN_TIME: Duration = Duration::from_secs(10);
// Audio sinks (A2DP), so a scan can say which devices take audio.
const A2DP_SINK: &str = "0000110b-0000-1000-8000-00805f9b34fb";

#[derive(Serialize)]
pub struct Adapter {
    powered: bool,
    name: String,
    address: String,
    discoverable: bool,
}

#[derive(Serialize)]
pub struct Device {
    address: String,
    name: String,
    connected: bool,
    /// The codec in use, when known.
    codec: Option<String>,
    audio: bool,
}

#[derive(Serialize)]
pub struct Found {
    address: String,
    name: Option<String>,
    codecs: Vec<String>,
    audio: bool,
}

#[derive(Serialize)]
pub struct Status {
    adapter: Adapter,
    paired: Vec<Device>,
}

async fn proxy<'a>(
    conn: &'a zbus::Connection,
    path: &'a str,
    iface: &'a str,
) -> Result<zbus::Proxy<'a>> {
    Ok(zbus::proxy::Builder::new(conn)
        .destination(BLUEZ)?
        .path(path.to_string())?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await?)
}

type Props = HashMap<String, OwnedValue>;

fn prop<T: TryFrom<OwnedValue>>(p: &Props, k: &str) -> Option<T> {
    p.get(k).and_then(|v| T::try_from(v.clone()).ok())
}

/// Every device BlueZ knows on the adapter: (path, properties).
async fn devices(conn: &zbus::Connection) -> Result<Vec<(String, Props)>> {
    let om = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination(BLUEZ)?
        .path("/")?
        .build()
        .await?;
    let objects = om
        .get_managed_objects()
        .await
        .map_err(|_| Error::new(StatusCode::SERVICE_UNAVAILABLE, "Bluetooth isn't running."))?;
    Ok(objects
        .into_iter()
        .filter(|(path, _)| path.as_str().starts_with(&format!("{}/", ADAPTER)))
        .filter_map(|(path, mut ifaces)| {
            let props = ifaces.remove("org.bluez.Device1")?;
            Some((
                path.to_string(),
                props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            ))
        })
        .collect())
}

fn is_audio(p: &Props) -> bool {
    prop::<Vec<String>>(p, "UUIDs")
        .map(|u| u.iter().any(|u| u == A2DP_SINK))
        .unwrap_or(false)
}

fn device_path(addr: &str) -> Result<String> {
    let ok = addr.len() == 17
        && addr
            .split(':')
            .all(|b| b.len() == 2 && b.chars().all(|c| c.is_ascii_hexdigit()));
    if !ok {
        return Err(Error::bad_request(format!(
            "{} isn't a Bluetooth address.",
            addr
        )));
    }
    Ok(format!(
        "{}/dev_{}",
        ADAPTER,
        addr.to_uppercase().replace(':', "_")
    ))
}

/// `GET /api/bluetooth`
pub async fn status(State(app): State<crate::State>) -> Result<Json<Status>> {
    let conn = app.system_bus().await?;
    let a = proxy(conn, ADAPTER, "org.bluez.Adapter1").await?;
    let adapter = Adapter {
        powered: a.get_property("Powered").await?,
        name: a.get_property("Alias").await?,
        address: a.get_property("Address").await?,
        discoverable: a.get_property("Discoverable").await?,
    };
    let mut paired: Vec<Device> = devices(conn)
        .await?
        .into_iter()
        .filter(|(_, p)| prop::<bool>(p, "Paired").unwrap_or(false))
        .map(|(_, p)| Device {
            address: prop(&p, "Address").unwrap_or_default(),
            name: prop::<String>(&p, "Alias")
                .or_else(|| prop(&p, "Name"))
                .unwrap_or_default(),
            connected: prop(&p, "Connected").unwrap_or(false),
            codec: None,
            audio: is_audio(&p),
        })
        .collect();
    paired.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.cmp(&b.name)));
    Ok(Json(Status { adapter, paired }))
}

/// `POST /api/bluetooth/scan`: about 10 s of discovery, then the unpaired
/// devices found, named ones first.
pub async fn scan(State(app): State<crate::State>) -> Result<Json<Vec<Found>>> {
    let conn = app.system_bus().await?;
    let a = proxy(conn, ADAPTER, "org.bluez.Adapter1").await?;
    a.call_method("StartDiscovery", &()).await?;
    tokio::time::sleep(SCAN_TIME).await;
    let _ = a.call_method("StopDiscovery", &()).await;
    let mut found: Vec<Found> = devices(conn)
        .await?
        .into_iter()
        .filter(|(_, p)| !prop::<bool>(p, "Paired").unwrap_or(false))
        .filter(|(_, p)| p.contains_key("RSSI"))
        .map(|(_, p)| Found {
            address: prop(&p, "Address").unwrap_or_default(),
            name: prop(&p, "Name"),
            // The codecs are only known after connecting.
            codecs: Vec::new(),
            audio: is_audio(&p),
        })
        .collect();
    found.sort_by(|a, b| {
        (a.name.is_none(), !a.audio, &a.name).cmp(&(b.name.is_none(), !b.audio, &b.name))
    });
    Ok(Json(found))
}

fn bluez_error(what: &str, e: zbus::Error) -> Error {
    let name = match &e {
        zbus::Error::MethodError(name, _, _) => name.as_str().to_string(),
        _ => String::new(),
    };
    let msg = match name.rsplit('.').next().unwrap_or("") {
        "AuthenticationFailed" | "AuthenticationRejected" | "AuthenticationCanceled" => {
            format!("{}: the device refused to pair.", what)
        }
        "ConnectionAttemptFailed" | "Failed" => {
            format!("{}: the device didn't answer. Is it on and in range?", what)
        }
        "AlreadyExists" => format!("{}: already paired.", what),
        "DoesNotExist" | "UnknownObject" => format!("{}: not found. Scan again?", what),
        _ => format!("{}: {}", what, e),
    };
    Error::bad_request(msg)
}

/// `POST /api/bluetooth/{addr}/pair`: pair, trust and connect.
pub async fn pair(
    State(app): State<crate::State>,
    UrlPath(addr): UrlPath<String>,
) -> Result<StatusCode> {
    let conn = app.system_bus().await?;
    let path = device_path(&addr)?;
    let d = proxy(conn, &path, "org.bluez.Device1").await?;
    info!("Pairing with {}", addr);
    if !d.get_property::<bool>("Paired").await.unwrap_or(false) {
        d.call_method("Pair", &())
            .await
            .map_err(|e| bluez_error("Pairing", e))?;
    }
    d.set_property("Trusted", true)
        .await
        .map_err(|e| Error::internal(format!("Trusting {}: {}", addr, e)))?;
    d.call_method("Connect", &())
        .await
        .map_err(|e| bluez_error("Connecting", e))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn device_call(app: &crate::App, addr: &str, method: &str, what: &str) -> Result<StatusCode> {
    let conn = app.system_bus().await?;
    let path = device_path(addr)?;
    let d = proxy(conn, &path, "org.bluez.Device1").await?;
    d.call_method(method, &())
        .await
        .map_err(|e| bluez_error(what, e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/bluetooth/{addr}/connect`
pub async fn connect(
    State(app): State<crate::State>,
    UrlPath(addr): UrlPath<String>,
) -> Result<StatusCode> {
    device_call(&app, &addr, "Connect", "Connecting").await
}

/// `POST /api/bluetooth/{addr}/disconnect`
pub async fn disconnect(
    State(app): State<crate::State>,
    UrlPath(addr): UrlPath<String>,
) -> Result<StatusCode> {
    device_call(&app, &addr, "Disconnect", "Disconnecting").await
}

/// `DELETE /api/bluetooth/{addr}`: forget the pairing.
pub async fn remove(
    State(app): State<crate::State>,
    UrlPath(addr): UrlPath<String>,
) -> Result<StatusCode> {
    let conn = app.system_bus().await?;
    let path = device_path(&addr)?;
    let a = proxy(conn, ADAPTER, "org.bluez.Adapter1").await?;
    a.call_method(
        "RemoveDevice",
        &(OwnedObjectPath::try_from(path).map_err(|e| Error::internal(e.to_string()))?,),
    )
    .await
    .map_err(|e| bluez_error("Removing", e))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    discoverable: bool,
}

/// `PATCH /api/bluetooth` `{ discoverable }`: BlueZ turns it off again after
/// its DiscoverableTimeout (3 minutes).
pub async fn set(State(app): State<crate::State>, Json(c): Json<Change>) -> Result<StatusCode> {
    let conn = app.system_bus().await?;
    let a = proxy(conn, ADAPTER, "org.bluez.Adapter1").await?;
    a.set_property("Discoverable", Value::from(c.discoverable))
        .await
        .map_err(|e| Error::internal(format!("Bluetooth: {}", e)))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths() {
        assert_eq!(
            device_path("e0:d4:e8:a3:c0:8b").unwrap(),
            "/org/bluez/hci0/dev_E0_D4_E8_A3_C0_8B"
        );
        assert!(device_path("e0:d4:e8:a3:c0").is_err());
        assert!(device_path("../../x").is_err());
    }
}
