//! The client build as the server sends it: each file in the precompressed encoding the browser
//! accepts, where the build wrote one beside it (`<file>.br`, `<file>.gz`), Vite's hashed bundle
//! under `assets/` cached for good, and every other file, such as the page, the cakes, and the
//! previews, revalidated by a tag of its content, so a browser downloads it again only once it
//! changes. The tags are read once, when the server starts, so a rebuilt client needs a restart.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hasher};
use std::io;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use tower_http::services::ServeDir;

/// Where Vite writes the bundle, every file named by a hash of its content.
const HASHED_PREFIX: &str = "/assets/";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Kept, but checked with the server before each use.
const REVALIDATED: &str = "no-cache";
const PRECOMPRESSED_EXTENSIONS: [&str; 2] = ["br", "gz"];

/// Each client file's tag, by the path a request names it with.
type ContentTags = BTreeMap<String, HeaderValue>;

/// Serves `client_dir`, reading every file in it once to tag its content.
pub fn build_router(client_dir: &Path) -> io::Result<Router> {
    let tags = Arc::new(build_tags(client_dir)?);
    let files = ServeDir::new(client_dir).precompressed_br().precompressed_gzip();
    Ok(Router::new().fallback_service(files).layer(from_fn_with_state(tags, add_cache_headers)))
}

/// Tags every file under `dir` but the precompressed variants, which share their original's tag,
/// and `/` with `index.html`'s, which serves it.
fn build_tags(dir: &Path) -> io::Result<ContentTags> {
    let mut tags = BTreeMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(next_dir) = dirs.pop() {
        for entry in std::fs::read_dir(&next_dir)? {
            let path = entry?.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path
                .extension()
                .is_some_and(|extension| PRECOMPRESSED_EXTENSIONS.iter().any(|known| extension == *known))
            {
                continue;
            }
            let relative = path.strip_prefix(dir).expect("a walked path lies in its dir");
            let url_path: Vec<String> = relative.iter().map(|part| part.to_string_lossy().into_owned()).collect();
            tags.insert(format!("/{}", url_path.join("/")), calc_tag(&std::fs::read(&path)?));
        }
    }
    if let Some(index) = tags.get("/index.html").cloned() {
        tags.insert("/".to_string(), index);
    }
    Ok(tags)
}

/// A weak tag, since the same content goes out in several encodings.
fn calc_tag(bytes: &[u8]) -> HeaderValue {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    HeaderValue::from_str(&format!("W/\"{:016x}\"", hasher.finish())).expect("a hex tag is a valid header")
}

/// Answers a request for a file the browser already holds with 304, and marks every file sent or
/// held with how long to keep it, its tag, and that its encoding follows `Accept-Encoding`.
async fn add_cache_headers(State(tags): State<Arc<ContentTags>>, mut request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let cache_control = HeaderValue::from_static(if path.starts_with(HASHED_PREFIX) { IMMUTABLE } else { REVALIDATED });
    let tag = tags.get(path).cloned();
    let headers = request.headers_mut();
    if let Some(tag) = &tag
        && is_held(headers, tag)
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        set_cache_headers(response.headers_mut(), cache_control, Some(tag.clone()));
        return response;
    }
    // A tag that differs outranks a date: the file changed, whatever its time says.
    if headers.contains_key(header::IF_NONE_MATCH) {
        headers.remove(header::IF_MODIFIED_SINCE);
    }
    let mut response = next.run(request).await;
    // A 304 by date alone, for a browser that cached the file before it had a tag, must carry the
    // tag and the caching rules too, or the browser keeps guessing how long the file stays fresh.
    if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED {
        set_cache_headers(response.headers_mut(), cache_control, tag);
    }
    response
}

/// Whether the request's `If-None-Match` names `tag`, compared weakly, as for a `GET`.
fn is_held(headers: &HeaderMap, tag: &HeaderValue) -> bool {
    let Some(held) = headers.get(header::IF_NONE_MATCH).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let opaque = |candidate: &str| candidate.trim().trim_start_matches("W/").to_string();
    let wanted = opaque(tag.to_str().expect("tags are ASCII"));
    held.split(',').any(|candidate| candidate.trim() == "*" || opaque(candidate) == wanted)
}

fn set_cache_headers(headers: &mut HeaderMap, cache_control: HeaderValue, tag: Option<HeaderValue>) {
    headers.insert(header::CACHE_CONTROL, cache_control);
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    if let Some(tag) = tag {
        headers.insert(header::ETAG, tag);
    }
}
