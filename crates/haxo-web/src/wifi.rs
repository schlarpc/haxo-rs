// Wi-Fi through iwd's D-Bus API. Saved networks are the settings file's
// [[wifi]] blocks; joining without saving lasts until the next reboot
// (iwd keeps what it learns on tmpfs).
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use log::info;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

use crate::error::{Error, Result};
use crate::settings;

const IWD: &str = "net.connman.iwd";
const AGENT_PATH: &str = "/xyz/haxophone/web/iwd_agent";

#[derive(Serialize, Clone)]
pub struct Network {
    ssid: String,
    /// dBm.
    signal: i32,
    security: String,
}

#[derive(Serialize)]
pub struct Current {
    ssid: String,
    signal: Option<i32>,
    address: Option<String>,
    security: String,
}

#[derive(Serialize)]
pub struct Status {
    current: Option<Current>,
    saved: Vec<Saved>,
}

#[derive(Serialize)]
pub struct Saved {
    ssid: String,
}

fn security(kind: &str) -> String {
    match kind {
        "open" => "open",
        "psk" => "WPA2/3",
        "8021x" => "802.1X",
        "wep" => "WEP",
        other => other,
    }
    .to_string()
}

async fn proxy<'a>(
    conn: &'a zbus::Connection,
    path: &'a str,
    iface: &'a str,
) -> Result<zbus::Proxy<'a>> {
    Ok(zbus::proxy::Builder::new(conn)
        .destination(IWD)?
        .path(path.to_string())?
        .interface(iface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await?)
}

/// The object path of the station on `interface`.
async fn station(conn: &zbus::Connection, interface: &str) -> Result<String> {
    let om = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination(IWD)?
        .path("/")?
        .build()
        .await?;
    let objects = om.get_managed_objects().await.map_err(|_| {
        Error::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Wi-Fi isn't running (iwd).",
        )
    })?;
    for (path, ifaces) in objects {
        let Some(dev) = ifaces.get("net.connman.iwd.Device") else {
            continue;
        };
        let name = dev
            .get("Name")
            .and_then(|v| String::try_from(v.clone()).ok());
        if name.as_deref() == Some(interface) && ifaces.contains_key("net.connman.iwd.Station") {
            return Ok(path.to_string());
        }
    }
    Err(Error::new(
        StatusCode::SERVICE_UNAVAILABLE,
        format!("No Wi-Fi station on {}.", interface),
    ))
}

/// Networks in range, strongest first: (object path, network).
async fn ordered(conn: &zbus::Connection, station_path: &str) -> Result<Vec<(String, Network)>> {
    let st = proxy(conn, station_path, "net.connman.iwd.Station").await?;
    let list: Vec<(OwnedObjectPath, i16)> = st.call("GetOrderedNetworks", &()).await?;
    let mut out = Vec::new();
    for (path, signal) in list {
        let n = proxy(conn, path.as_str(), "net.connman.iwd.Network").await?;
        let ssid: String = n.get_property("Name").await?;
        let kind: String = n.get_property("Type").await?;
        out.push((
            path.to_string(),
            Network {
                ssid,
                signal: signal as i32 / 100,
                security: security(&kind),
            },
        ));
    }
    Ok(out)
}

async fn current(app: &crate::App) -> Result<Option<Current>> {
    let conn = app.system_bus().await?;
    let st_path = station(conn, &app.args.wifi_interface).await?;
    let st = proxy(conn, &st_path, "net.connman.iwd.Station").await?;
    let state: String = st.get_property("State").await?;
    if state != "connected" {
        return Ok(None);
    }
    let net_path: OwnedObjectPath = st.get_property("ConnectedNetwork").await?;
    let n = proxy(conn, net_path.as_str(), "net.connman.iwd.Network").await?;
    let ssid: String = n.get_property("Name").await?;
    let kind: String = n.get_property("Type").await?;
    let signal = ordered(conn, &st_path)
        .await
        .ok()
        .and_then(|l| l.into_iter().find(|(p, _)| p == net_path.as_str()))
        .map(|(_, n)| n.signal);
    let address = crate::auth::ipv4_addresses(&app.args.wifi_interface)
        .first()
        .map(|(ip, _)| ip.to_string());
    Ok(Some(Current {
        ssid,
        signal,
        address,
        security: security(&kind),
    }))
}

/// `GET /api/wifi`
pub async fn status(State(app): State<crate::State>) -> Result<Json<Status>> {
    let saved = settings::show(&app.args)
        .await?
        .wifi
        .into_iter()
        .map(|w| Saved { ssid: w.ssid })
        .collect();
    Ok(Json(Status {
        current: current(&app).await?,
        saved,
    }))
}

/// `POST /api/wifi/scan`: scan, then the networks in range.
pub async fn scan(State(app): State<crate::State>) -> Result<Json<Vec<Network>>> {
    let conn = app.system_bus().await?;
    let st_path = station(conn, &app.args.wifi_interface).await?;
    let st = proxy(conn, &st_path, "net.connman.iwd.Station").await?;
    // Busy (already scanning) is fine: wait for that scan instead.
    let _ = st.call_method("Scan", &()).await;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let scanning: bool = st.get_property("Scanning").await?;
        if !scanning {
            break;
        }
    }
    let mut seen = std::collections::HashSet::new();
    Ok(Json(
        ordered(conn, &st_path)
            .await?
            .into_iter()
            .map(|(_, n)| n)
            .filter(|n| seen.insert(n.ssid.clone()))
            .collect(),
    ))
}

// iwd asks its agent for the passphrase of a network it doesn't know yet.
// The one for the network being joined is waiting here, by network path.
#[derive(Clone, Default)]
struct Agent {
    passphrases: Arc<Mutex<HashMap<String, String>>>,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl Agent {
    fn release(&self) {}

    fn request_passphrase(&self, network: OwnedObjectPath) -> zbus::fdo::Result<String> {
        self.passphrases
            .lock()
            .unwrap()
            .get(network.as_str())
            .cloned()
            .ok_or_else(|| zbus::fdo::Error::Failed("no passphrase given".into()))
    }

    fn request_private_key_passphrase(
        &self,
        _network: OwnedObjectPath,
    ) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::NotSupported(
            "802.1X isn't supported".into(),
        ))
    }

    fn request_user_name_and_password(
        &self,
        _network: OwnedObjectPath,
    ) -> zbus::fdo::Result<(String, String)> {
        Err(zbus::fdo::Error::NotSupported(
            "802.1X isn't supported".into(),
        ))
    }

    fn request_user_password(
        &self,
        _network: OwnedObjectPath,
        _user: String,
    ) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::NotSupported(
            "802.1X isn't supported".into(),
        ))
    }

    fn cancel(&self, _reason: String) {}
}

static AGENT: OnceCell<Agent> = OnceCell::const_new();

async fn agent(conn: &zbus::Connection) -> Result<&'static Agent> {
    AGENT
        .get_or_try_init(|| async {
            let agent = Agent::default();
            conn.object_server().at(AGENT_PATH, agent.clone()).await?;
            let mgr = proxy(conn, "/net/connman/iwd", "net.connman.iwd.AgentManager").await?;
            mgr.call_method(
                "RegisterAgent",
                &(zbus::zvariant::ObjectPath::try_from(AGENT_PATH).unwrap(),),
            )
            .await?;
            info!("Registered the iwd agent");
            Ok::<_, Error>(agent)
        })
        .await
}

#[derive(Deserialize)]
pub struct Connect {
    ssid: String,
    psk: Option<String>,
    #[serde(default)]
    save: bool,
    /// The person was told this may cut off their connection to the board.
    #[serde(default)]
    disconnect_ok: bool,
}

// Joining another network, or forgetting the one in use, drops the board's
// Wi-Fi link, and with it a page that came in over Wi-Fi. Refuse until the
// person has said they know; over the USB cable there's nothing to lose.
fn guard(app: &crate::App, peer: SocketAddr, confirmed: bool, what: &str) -> Result<()> {
    if confirmed || !app.auth.over_wifi(peer) {
        return Ok(());
    }
    Err(Error::needs_confirming(
        "disconnect",
        format!(
            "You're connected to the haxophone over Wi-Fi. {} will drop this \
             connection, and the page may not come back until you join the \
             same network as the haxophone (or connect the USB cable).",
            what
        ),
    ))
}

fn connect_error(ssid: &str, e: zbus::Error) -> Error {
    let name = match &e {
        zbus::Error::MethodError(name, _, _) => name.as_str().to_string(),
        _ => String::new(),
    };
    let msg = match name.rsplit('.').next().unwrap_or("") {
        "Failed" | "InvalidFormat" => {
            format!("{} rejected the passphrase. Check it and try again.", ssid)
        }
        "NotSupported" => format!("{} needs a login (802.1X), which isn't supported.", ssid),
        "Aborted" | "Timeout" => format!("Joining {} timed out. Try again closer to it.", ssid),
        "NotFound" => format!("{} isn't in range.", ssid),
        _ => format!("Couldn't join {}: {}", ssid, e),
    };
    Error::bad_request(msg)
}

/// `POST /api/wifi/connect` `{ ssid, psk?, save }`
pub async fn connect(
    State(app): State<crate::State>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Connect>,
) -> Result<Json<Current>> {
    if let Some(psk) = &body.psk {
        if !(8..=63).contains(&psk.len()) {
            return Err(Error::bad_request(
                "The passphrase needs 8 to 63 characters.",
            ));
        }
    }
    let conn = app.system_bus().await?;
    let st_path = station(conn, &app.args.wifi_interface).await?;
    let networks = ordered(conn, &st_path).await?;
    let Some((path, net)) = networks.into_iter().find(|(_, n)| n.ssid == body.ssid) else {
        return Err(Error::bad_request(format!(
            "{} isn't in range. Scan again?",
            body.ssid
        )));
    };
    if net.security == "802.1X" {
        return Err(Error::bad_request(format!(
            "{} needs a login (802.1X), which isn't supported.",
            body.ssid
        )));
    }
    if net.security != "open" && body.psk.is_none() {
        return Err(Error::bad_request(format!(
            "{} needs a passphrase.",
            body.ssid
        )));
    }
    let joined = current(&app).await.ok().flatten().map(|c| c.ssid);
    if joined.as_deref() != Some(body.ssid.as_str()) {
        guard(
            &app,
            peer,
            body.disconnect_ok,
            &format!("Joining {}", body.ssid),
        )?;
    }
    if body.save {
        settings::wifi_add(&app.args, &body.ssid, body.psk.as_deref()).await?;
        settings::reapply(&app.args).await?;
    }
    let agent = agent(conn).await?;
    if let Some(psk) = &body.psk {
        agent
            .passphrases
            .lock()
            .unwrap()
            .insert(path.clone(), psk.clone());
    }
    info!("Joining {}", body.ssid);
    let n = proxy(conn, &path, "net.connman.iwd.Network").await?;
    let result = n.call_method("Connect", &()).await;
    agent.passphrases.lock().unwrap().remove(&path);
    result.map_err(|e| connect_error(&body.ssid, e))?;
    current(&app)
        .await?
        .map(Json)
        .ok_or_else(|| Error::internal("Joined, but not connected."))
}

/// `DELETE /api/wifi/saved/{ssid}`
#[derive(Deserialize)]
pub struct ForgetQuery {
    #[serde(default)]
    disconnect_ok: bool,
}

pub async fn forget(
    State(app): State<crate::State>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    UrlPath(ssid): UrlPath<String>,
    Query(q): Query<ForgetQuery>,
) -> Result<axum::http::StatusCode> {
    let joined = current(&app).await.ok().flatten().map(|c| c.ssid);
    if joined.as_deref() == Some(ssid.as_str()) {
        guard(&app, peer, q.disconnect_ok, &format!("Forgetting {}", ssid))?;
    }
    settings::wifi_remove(&app.args, &ssid).await?;
    settings::reapply(&app.args).await?;
    Ok(StatusCode::NO_CONTENT)
}
