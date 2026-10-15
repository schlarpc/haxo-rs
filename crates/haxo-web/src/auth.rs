// Who may use the API: anyone on the USB link (whoever has the cable has
// the instrument), and over anything else whoever logged in with the
// password from the settings file ([web] password). With no password set,
// only the USB link gets in.
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use log::info;
use rand::RngCore;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::settings;

const COOKIE: &str = "haxo_session";

pub struct Auth {
    usb_interface: String,
    sessions: Mutex<HashSet<String>>,
}

impl Auth {
    pub fn new(usb_interface: &str) -> Self {
        Auth {
            usb_interface: usb_interface.to_string(),
            sessions: Mutex::new(HashSet::new()),
        }
    }

    // Did this connection come in over the USB link?
    pub fn trusted(&self, peer: SocketAddr) -> bool {
        match peer {
            SocketAddr::V4(v4) => on_link(&self.usb_interface, *v4.ip()),
            SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
                Some(v4) => on_link(&self.usb_interface, v4),
                // Link-local, scoped to the USB interface.
                None => {
                    (v6.ip().segments()[0] & 0xffc0) == 0xfe80
                        && v6.scope_id() != 0
                        && v6.scope_id() == if_index(&self.usb_interface)
                }
            },
        }
    }

    /// Whether this connection rides on Wi-Fi: neither the USB link nor this
    /// machine itself. Changing Wi-Fi can cut it off.
    pub fn over_wifi(&self, peer: SocketAddr) -> bool {
        let ip = match peer {
            SocketAddr::V4(v4) => std::net::IpAddr::V4(*v4.ip()),
            SocketAddr::V6(v6) => v6
                .ip()
                .to_ipv4_mapped()
                .map(std::net::IpAddr::V4)
                .unwrap_or(std::net::IpAddr::V6(*v6.ip())),
        };
        !self.trusted(peer) && !ip.is_loopback()
    }

    fn has_session(&self, headers: &HeaderMap) -> bool {
        let Some(token) = cookie(headers, COOKIE) else {
            return false;
        };
        self.sessions.lock().unwrap().contains(token)
    }
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

fn if_index(name: &str) -> u32 {
    let Ok(c) = std::ffi::CString::new(name) else {
        return 0;
    };
    unsafe { libc::if_nametoindex(c.as_ptr()) }
}

/// The IPv4 addresses of `interface`, with their prefix lengths.
pub fn ipv4_addresses(interface: &str) -> Vec<(Ipv4Addr, u32)> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut p = ifap;
    while !p.is_null() {
        let ifa = unsafe { &*p };
        p = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_netmask.is_null() {
            continue;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) };
        if name.to_bytes() != interface.as_bytes() {
            continue;
        }
        if unsafe { (*ifa.ifa_addr).sa_family } as i32 != libc::AF_INET {
            continue;
        }
        let addr = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
        let mask = unsafe { &*(ifa.ifa_netmask as *const libc::sockaddr_in) };
        let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
        let prefix = u32::from_be(mask.sin_addr.s_addr).count_ones();
        out.push((ip, prefix));
    }
    unsafe { libc::freeifaddrs(ifap) };
    out
}

fn on_link(interface: &str, peer: Ipv4Addr) -> bool {
    ipv4_addresses(interface).iter().any(|(ip, prefix)| {
        let mask = if *prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        (u32::from(*ip) & mask) == (u32::from(peer) & mask)
    })
}

/// Middleware for everything under /api but the session calls.
pub async fn require(
    State(app): State<crate::State>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if app.auth.trusted(peer) || app.auth.has_session(req.headers()) {
        return next.run(req).await;
    }
    Error::new(StatusCode::UNAUTHORIZED, "Log in first.").into_response()
}

/// `GET /api/session`: what the page needs to decide whether to ask for
/// the password.
pub async fn session(
    State(app): State<crate::State>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>> {
    let trusted = app.auth.trusted(peer);
    let password_set = settings::show(&app.args).await?.web.password_set;
    Ok(Json(serde_json::json!({
        "trusted": trusted,
        "authenticated": trusted || app.auth.has_session(&headers),
        "password_set": password_set,
        // How this page reaches the board: "wifi" can be cut off by Wi-Fi
        // changes.
        "via": if app.auth.over_wifi(peer) { "wifi" } else if trusted { "usb" } else { "other" },
    })))
}

#[derive(Deserialize)]
pub struct Login {
    password: String,
}

/// `POST /api/login` `{ password }`: sets the session cookie.
pub async fn login(
    State(app): State<crate::State>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Login>,
) -> Result<Response> {
    let (set, matches) = settings::check_password(&app.args, &body.password).await?;
    if !set {
        return Err(Error::new(
            StatusCode::FORBIDDEN,
            "Web access over Wi-Fi is off: set a password under [web] in haxophone.toml, \
             or connect over the USB cable.",
        ));
    }
    if !matches {
        // Slow down guessing.
        tokio::time::sleep(Duration::from_secs(1)).await;
        info!("Failed login from {}", peer.ip());
        return Err(Error::new(
            StatusCode::UNAUTHORIZED,
            "That's not the password.",
        ));
    }
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    app.auth.sessions.lock().unwrap().insert(token.clone());
    info!("Login from {}", peer.ip());
    Ok((
        StatusCode::NO_CONTENT,
        [(
            header::SET_COOKIE,
            format!("{}={}; Path=/; HttpOnly; SameSite=Strict", COOKIE, token),
        )],
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cookies() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            "a=1; haxo_session=abc; b=2".parse().unwrap(),
        );
        assert_eq!(cookie(&h, COOKIE), Some("abc"));
        assert_eq!(cookie(&h, "c"), None);
    }

    #[test]
    fn loopback_is_on_lo() {
        let a = Auth::new("lo");
        assert!(a.trusted("127.0.0.1:5".parse().unwrap()));
        assert!(!a.trusted("192.0.2.1:5".parse().unwrap()));
        let b = Auth::new("no-such-if");
        assert!(!b.trusted("127.0.0.1:5".parse().unwrap()));
    }
}
