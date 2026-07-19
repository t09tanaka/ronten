//! Frontend assets embedded into the binary at compile time.
//!
//! The Svelte SPA is built by Vite into `frontend/dist` (see `build.rs`), and
//! this module embeds that directory into the binary via `rust-embed` so the
//! server can be shipped as a single self-contained executable.

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist"]
pub struct Asset;

/// Look up `path` in the embedded frontend assets and build a response for
/// it. Returns the file body with a `mime_guess`ed `Content-Type` header if
/// found, or an empty 404 response otherwise.
pub fn serve(path: &str) -> Response {
    match Asset::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref().to_string())],
                Body::from(file.data.into_owned()),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, Body::empty()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_html_is_embedded() {
        assert!(Asset::get("index.html").is_some());
    }

    // The bundled Shippori Mincho subsets are OFL-licensed; the license text
    // must ship with the font in every distribution, including the single
    // self-contained binary.
    #[test]
    fn font_license_is_embedded() {
        assert!(Asset::get("shippori-mincho-OFL.txt").is_some());
    }
}
