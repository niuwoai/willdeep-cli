//! Read-only package resources for desktop's opaque-origin plugin iframes.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use uuid::Uuid;
use willdeep_core::plugin::package::MAX_PAGE_BYTES;
use willdeep_core::plugin::{HostError, PageRuntime, PluginApproval, PluginHost, PluginPackage};

use super::{PluginWebError, PluginWebState, apply_sandbox_cors, mime_for};

const ROUTE_PREFIX: &str = "/__willdeep_plugin_assets";
const ASSET_ROUTE: &str = "/__willdeep_plugin_assets/{plugin}/{token}/{*path}";

#[derive(Default)]
pub(super) struct DesktopAssets {
    grants: Mutex<BTreeMap<(String, PathBuf), Arc<AssetGrant>>>,
}

struct ApprovedPage {
    source: Vec<u8>,
    hashes: BTreeMap<String, [u8; 32]>,
}

struct AssetGrant {
    token: Uuid,
    package_root: PathBuf,
    directory: PathBuf,
    approval: PluginApproval,
    activation: Option<Uuid>,
    hashes: BTreeMap<String, [u8; 32]>,
}

impl AssetGrant {
    fn same_approval(&self, other: &Self) -> bool {
        self.package_root == other.package_root
            && self.activation == other.activation
            && self.approval == other.approval
    }

    fn is_current(&self, host: &PluginHost, plugin: &str) -> Result<bool, HostError> {
        let registry = host.registry_snapshot()?;
        Ok(registry.state(plugin).is_some_and(|entry| {
            entry.enabled
                && entry.activation == self.activation
                && entry.approval.as_ref() == Some(&self.approval)
        }) && host.package(plugin).is_ok_and(|package| {
            package.root == self.package_root
                && package.version == self.approval.version
                && package.source.as_str() == self.approval.source
        }))
    }
}

impl DesktopAssets {
    /// Call only from the cookie-protected route, after checking the declared entry
    /// and its current package approval. A grant can only address files below the
    /// entry directory, never the rest of the host or another plugin.
    async fn issue(
        &self,
        host: &PluginHost,
        plugin: &str,
        mut grant: AssetGrant,
    ) -> Result<Option<String>, HostError> {
        let mut grants = self.grants.lock().await;
        // The package digest ran off-thread. A stale request must not replace a
        // grant issued by a more recent enablement or package approval.
        if !grant.is_current(host, plugin)? {
            return Ok(None);
        }
        grants.retain(|(id, _), current| id != plugin || current.same_approval(&grant));
        let key = (plugin.to_owned(), grant.directory.clone());
        if let Some(current) = grants.get(&key) {
            grant.token = current.token;
        }
        let token = grant.token.simple();
        grants.insert(key, Arc::new(grant));
        Ok(Some(format!(
            "{ROUTE_PREFIX}/{}/{token}/",
            super::urlencoding(plugin)
        )))
    }
}

pub(super) async fn page(
    state: &PluginWebState,
    assets: &DesktopAssets,
    host: &PluginHost,
    package: Arc<PluginPackage>,
    path: &str,
    headers: &HeaderMap,
) -> Result<Response, PluginWebError> {
    let entry = package.manifest.as_ref().is_some_and(|manifest| {
        manifest.pages.iter().any(|page| {
            page.runtime == PageRuntime::LocalWeb
                && page
                    .entry_path
                    .as_deref()
                    .and_then(super::package_url_path)
                    .as_deref()
                    == Some(path)
        })
    });
    if !entry || !safe_path(path) || mime_for(path) != "text/html" {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let registry = host.registry_snapshot().map_err(HostError::from)?;
    let plugin = &package.id;
    let Some((enabled, approval)) = registry
        .state(plugin)
        .filter(|entry| entry.enabled)
        .and_then(|entry| Some((entry, entry.approval.as_ref()?)))
    else {
        return Ok(StatusCode::CONFLICT.into_response());
    };
    let activation = enabled.activation;
    let approval = approval.clone();
    let prefix = path.rfind('/').map_or("", |end| &path[..=end]);
    let directory = package
        .root
        .canonicalize()
        .map_err(|error| {
            PluginWebError::Internal(format!("cannot resolve plugin package {plugin}: {error}"))
        })?
        .join(prefix);
    let digest = package.clone();
    let entry_path = path.to_owned();
    let prefix = prefix.to_owned();
    // The HTML and file hashes come from a single authoritative package digest.
    // Each import can then validate just its own bytes against that approval.
    let approved = tokio::task::spawn_blocking(move || {
        let mut page = ApprovedPage {
            source: Vec::new(),
            hashes: BTreeMap::new(),
        };
        let mut found = false;
        let accepted = registry
            .approval_gap_with_contents(&digest, |relative, bytes| {
                if bytes.len() as u64 > MAX_PAGE_BYTES {
                    return;
                }
                if relative == entry_path {
                    page.source.extend_from_slice(bytes);
                    found = true;
                } else if let Some(asset) = relative.strip_prefix(&prefix)
                    && static_mime(asset).is_some()
                {
                    page.hashes
                        .insert(asset.to_owned(), Sha256::digest(bytes).into());
                }
            })?
            .is_none();
        Ok::<_, willdeep_core::plugin::RegistryError>((accepted && found).then_some(page))
    })
    .await
    .map_err(|error| PluginWebError::Internal(format!("plugin entry validation failed: {error}")))?
    .map_err(HostError::from)?;
    let Some(approved) = approved else {
        return Ok(StatusCode::CONFLICT.into_response());
    };
    let source = String::from_utf8(approved.source)
        .map_err(|_| PluginWebError::BadRequest("page is not valid UTF-8".to_owned()))?;
    let Some(base) = assets
        .issue(
            host,
            plugin,
            AssetGrant {
                token: Uuid::new_v4(),
                package_root: package.root.clone(),
                directory,
                approval,
                activation,
                hashes: approved.hashes,
            },
        )
        .await?
    else {
        return Ok(StatusCode::CONFLICT.into_response());
    };
    // The desktop protection layer has already matched this Host against its
    // private listener. Only the injected base is permitted by this document's CSP.
    let host_header = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .ok_or_else(|| PluginWebError::BadRequest("missing desktop host".to_owned()))?;
    let origin = format!("http://{host_header}");
    let asset_source = format!("{origin}{base}");
    // These two existing media routes still require ordinary desktop auth.
    let media_sources =
        format!("{asset_source} {origin}/plugin-media/ {origin}/plugin-production/");
    // Keep the same explicit download grant as the parent iframe.
    let sandbox = if plugin == "willdeep-video-studio" {
        "allow-scripts allow-downloads"
    } else {
        "allow-scripts"
    };
    let storage = super::read_storage(&state.home, plugin);
    Ok(super::html_response_with_policy(
        super::compose_page_with_base(&source, &storage, Some(&base)),
        headers,
        super::content_security_policy_for(
            &asset_source,
            &media_sources,
            &asset_source,
            &format!("; sandbox {sandbox}; frame-ancestors {origin}"),
        ),
    ))
}

/// The authenticated shell renders these images in native navigation. These
/// URLs stay on the cookie-protected router and never issue an asset grant.
pub(super) async fn navigation_icon(
    host: &PluginHost,
    package: Arc<PluginPackage>,
    path: &str,
) -> Result<Response, PluginWebError> {
    let mime = mime_for(path);
    let declared = package.manifest.as_ref().is_some_and(|manifest| {
        manifest
            .destinations
            .iter()
            .filter_map(|item| item.icon.as_deref())
            .chain(
                manifest
                    .commands
                    .iter()
                    .filter_map(|item| item.icon.as_deref()),
            )
            .filter_map(|icon| super::package_url_path(icon.trim()))
            .any(|icon| icon == path)
    });
    if !safe_path(path) || !mime.starts_with("image/") || !declared {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let registry = host.registry_snapshot().map_err(HostError::from)?;
    if !registry.is_enabled(&package.id) {
        return Err(HostError::NotEnabled(package.id.clone()).into());
    }
    let path = path.to_owned();
    let plugin = package.id.clone();
    let bytes = tokio::task::spawn_blocking(move || {
        let mut bytes = None;
        let gap = registry
            .approval_gap_with_contents(&package, |relative, content| {
                if relative == path && content.len() as u64 <= MAX_PAGE_BYTES {
                    bytes = Some(content.to_vec());
                }
            })
            .map_err(HostError::from)?;
        if gap.is_some() {
            return Err(HostError::PluginChanged(plugin));
        }
        Ok(bytes)
    })
    .await
    .map_err(|error| {
        PluginWebError::Internal(format!("plugin icon validation failed: {error}"))
    })??;
    let Some(bytes) = bytes else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'",
            ),
            (
                header::HeaderName::from_static("cross-origin-resource-policy"),
                "same-origin",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Clone)]
struct AssetState {
    server: Arc<PluginWebState>,
    host: String,
}

pub(crate) fn router(state: Arc<PluginWebState>, address: SocketAddr) -> Router {
    Router::new()
        .route(ASSET_ROUTE, get(serve))
        .with_state(AssetState {
            server: state,
            host: address.to_string(),
        })
}

async fn serve(
    State(state): State<AssetState>,
    Path((plugin, token, path)): Path<(String, Uuid, String)>,
    headers: HeaderMap,
) -> Result<Response, PluginWebError> {
    let same_origin = format!("http://{}", state.host);
    if headers
        .get(header::HOST)
        .is_none_or(|host| host != state.host.as_str())
        || headers
            .get(header::ORIGIN)
            .is_some_and(|origin| origin != "null" && origin != same_origin.as_str())
    {
        return Ok(StatusCode::FORBIDDEN.into_response());
    }
    let Some(mime) = static_mime(&path) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some(assets) = state.server.desktop_assets.as_ref() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let grant = assets
        .grants
        .lock()
        .await
        .iter()
        .find_map(|((id, _), grant)| {
            (id == &plugin && grant.token == token).then(|| grant.clone())
        });
    let Some(grant) = grant else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let host = state.server.host().await?;
    if !grant.is_current(&host, &plugin)? {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let Some(expected_hash) = grant.hashes.get(&path) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let target = grant.directory.join(FsPath::new(&path));
    // Check the resolved path too: symlinks and Windows alternate paths must not
    // turn a granted resource URL into a read from another directory.
    if !tokio::fs::canonicalize(&target)
        .await
        .is_ok_and(|resolved| resolved == target)
        || !tokio::fs::metadata(&target)
            .await
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_PAGE_BYTES)
    {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let Ok(file) = tokio::fs::File::open(&target).await else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_PAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .is_err()
        || bytes.len() as u64 > MAX_PAGE_BYTES
        || &<[u8; 32]>::from(Sha256::digest(&bytes)) != expected_hash
    {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let mut response = (
        [
            (header::CONTENT_TYPE, mime),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'",
            ),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        bytes,
    )
        .into_response();
    apply_sandbox_cors(&mut response, &headers);
    Ok(response)
}

fn safe_path(path: &str) -> bool {
    !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'))
}

fn static_mime(path: &str) -> Option<&'static str> {
    if !safe_path(path) {
        return None;
    }
    let mime = mime_for(path);
    // The capability cannot open HTML (which carries the bridge/storage data),
    // manifests, arbitrary bytes, or any of the desktop host's writable paths.
    matches!(
        mime,
        "text/javascript; charset=utf-8"
            | "text/css; charset=utf-8"
            | "application/wasm"
            | "image/svg+xml"
            | "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "image/x-icon"
            | "font/woff"
            | "font/woff2"
            | "font/ttf"
            | "font/otf"
            | "audio/mpeg"
            | "audio/ogg"
            | "audio/wav"
    )
    .then_some(mime)
}
