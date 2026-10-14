// Errors as HTTP responses: a status and `{ "error": "..." }`, the message
// written for the person reading the page.
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use log::warn;

#[derive(Debug)]
pub struct Error {
    pub status: StatusCode,
    pub message: String,
}

impl Error {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Error {
            status,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Error::new(StatusCode::BAD_REQUEST, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Error::new(StatusCode::NOT_FOUND, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Error::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            warn!("{}", self.message);
        }
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

impl From<zbus::Error> for Error {
    fn from(e: zbus::Error) -> Self {
        Error::internal(format!("D-Bus: {}", e))
    }
}

impl From<zbus::fdo::Error> for Error {
    fn from(e: zbus::fdo::Error) -> Self {
        Error::internal(format!("D-Bus: {}", e))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::internal(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
