//! Serves the SPA (web/dist embedded at build time) with index.html fallback, and avatars.
use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Assets;

pub async fn spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(f) = Assets::get(path).filter(|_| !path.is_empty()) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
        return Response::builder()
            .header(header::CONTENT_TYPE, mime.as_ref())
            .header(header::CACHE_CONTROL, cache)
            .body(Body::from(f.data.into_owned()))
            .unwrap();
    }
    match Assets::get("index.html") {
        Some(f) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(f.data.into_owned()))
            .unwrap(),
        None => (StatusCode::OK, "forge is running; the web UI has not been built (cd web && npm run build).").into_response(),
    }
}

/// Deterministic identicon-style SVG avatar for any login.
pub async fn avatar(axum::extract::Path(login): axum::extract::Path<String>) -> Response {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(login.to_lowercase().as_bytes());
    let color = format!("#{:02x}{:02x}{:02x}", h[0], h[1], h[2]);
    let mut cells = String::new();
    for y in 0..5 {
        for x in 0..3 {
            if h[3 + y * 3 + x] % 2 == 0 {
                for xx in [x, 4 - x] {
                    cells.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"1\" height=\"1\"/>", xx, y));
                }
            }
        }
    }
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-0.5 -0.5 6 6\" width=\"420\" height=\"420\"><rect x=\"-0.5\" y=\"-0.5\" width=\"6\" height=\"6\" fill=\"#f0f0f0\"/><g fill=\"{color}\">{cells}</g></svg>"
    );
    Response::builder()
        .header(header::CONTENT_TYPE, "image/svg+xml")
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(svg))
        .unwrap()
}
