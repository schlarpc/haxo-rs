//! haxo-web: the Haxophone's web UI (ui/index.html) and the API behind it
//! (docs/webui.md in haxo-nix). Live state and instrument settings go
//! through haxo001's control socket, Wi-Fi through iwd and Bluetooth
//! through BlueZ over D-Bus, and settings that outlive a boot through
//! `haxophone-config`, which edits the settings file on the card.

mod audio;
mod auth;
mod bluetooth;
mod error;
mod live;
mod settings;
mod sf2;
mod sounds;
mod wifi;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::response::Html;
use axum::routing::{delete, get, patch, post, put};
use axum::{middleware, Router};
use clap::Parser;
use log::info;

#[derive(Parser, Debug)]
#[command(version, about = "The Haxophone's web UI")]
pub struct Args {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:80")]
    pub listen: SocketAddr,
    /// haxo001's control socket.
    #[arg(long, default_value = "/run/haxo/control.sock")]
    pub control_socket: PathBuf,
    /// haxo001's audio tap.
    #[arg(long, default_value = "/run/haxo/audio.sock")]
    pub audio_socket: PathBuf,
    /// The settings helper's socket (`haxophone-config serve`).
    #[arg(long, default_value = "/run/haxophone-config/settings.sock")]
    pub settings_socket: PathBuf,
    /// Where uploaded soundfonts go (and the MTP storage).
    #[arg(long, default_value = "/var/lib/haxophone/files")]
    pub files_dir: PathBuf,
    /// A soundfont that comes with the system; can't be deleted. The first
    /// is what plays when the soundfont in use is deleted.
    #[arg(long = "builtin-soundfont")]
    pub builtin_soundfonts: Vec<PathBuf>,
    /// The network interface of the USB link. Requests over it need no
    /// password: whoever has the cable has the instrument.
    #[arg(long, default_value = "usb0")]
    pub usb_interface: String,
    /// The Wi-Fi interface (for its address).
    #[arg(long, default_value = "wlan0")]
    pub wifi_interface: String,
}

pub struct App {
    pub args: Args,
    pub live: live::Live,
    pub audio: audio::Audio,
    pub auth: auth::Auth,
    pub sf2: sf2::Cache,
    pub dbus: tokio::sync::OnceCell<zbus::Connection>,
}

impl App {
    pub async fn system_bus(&self) -> Result<&zbus::Connection, error::Error> {
        self.dbus
            .get_or_try_init(zbus::Connection::system)
            .await
            .map_err(error::Error::from)
    }
}

pub type State = Arc<App>;

// The page has no <!doctype> or <html> of its own (so it can also be
// previewed as an Artifact); wrap it here. Its <title> and <style> come
// first, so the head is left open for them and the parser closes it.
const PAGE: &str = include_str!("../ui/index.html");

async fn page() -> Html<String> {
    Html(format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n{}",
        PAGE
    ))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,zbus=warn"))
        .init();
    let args = Args::parse();
    let listen = args.listen;
    let app: State = Arc::new(App {
        live: live::Live::start(args.control_socket.clone()),
        audio: audio::Audio::start(args.audio_socket.clone()),
        auth: auth::Auth::new(&args.usb_interface),
        sf2: sf2::Cache::default(),
        dbus: tokio::sync::OnceCell::new(),
        args,
    });

    let api = Router::new()
        .route("/live", get(live::websocket))
        .route("/audio", get(audio::websocket))
        .route("/stream.wav", get(audio::wav))
        .route("/wifi", get(wifi::status))
        .route("/wifi/scan", post(wifi::scan))
        .route("/wifi/connect", post(wifi::connect))
        .route("/wifi/saved/{ssid}", delete(wifi::forget))
        .route("/bluetooth", get(bluetooth::status).patch(bluetooth::set))
        .route("/bluetooth/scan", post(bluetooth::scan))
        .route("/bluetooth/{addr}/pair", post(bluetooth::pair))
        .route("/bluetooth/{addr}/connect", post(bluetooth::connect))
        .route("/bluetooth/{addr}/disconnect", post(bluetooth::disconnect))
        .route("/bluetooth/{addr}", delete(bluetooth::remove))
        .route("/soundfonts", get(sounds::status))
        .route(
            "/soundfonts/{name}",
            put(sounds::upload).delete(sounds::remove),
        )
        .route("/instrument", patch(sounds::update))
        .route_layer(middleware::from_fn_with_state(app.clone(), auth::require))
        // Reachable without a session: how to get one.
        .route("/session", get(auth::session))
        .route("/login", post(auth::login));

    let router = Router::new()
        .route("/", get(page))
        .nest("/api", api)
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(listen).await?;
    info!("Listening on {}", listen);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}
