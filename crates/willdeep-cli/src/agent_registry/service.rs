//! Customer identity is independent from inference credentials.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, time::Duration};
use willdeep_core::agent_definition::store::{AgentLibrary, Origin, private_write};
mod resolution;
mod trusted_keys;
#[cfg(test)]
use resolution::resolve;
pub use resolution::{Conflict, ResolveChoice, resolve_scoped};
use willdeep_core::agent_definition::{AgentPackage, MAX_PACKAGE_BYTES, valid_hash};

pub const DEFAULT_ORIGIN: &str = "https://some.im";
// A complete account snapshot may contain 128 packages of up to 256 KiB each.
// Keep a bounded envelope allowance shared with the Mac transport.
const MAX_RESPONSE_BYTES: usize = 40 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct CustomerSession {
    pub origin: String,
    pub token: String,
    pub user_id: String,
    pub tenant_id: String,
}
impl CustomerSession {
    pub fn load(home: &Path) -> Result<Self> {
        serde_json::from_slice(&std::fs::read(home.join("agent-session.json"))?)
            .context("customer login required; inference API keys cannot access private agents")
    }
    pub fn library(&self, home: &Path) -> AgentLibrary {
        AgentLibrary::new(home).account_scope(&format!(
            "{}\n{}\n{}",
            self.origin, self.user_id, self.tenant_id
        ))
    }
    pub fn save(&self, home: &Path) -> Result<()> {
        ensure!(
            !self.token.trim().is_empty() && !self.user_id.is_empty() && !self.tenant_id.is_empty(),
            "incomplete customer identity"
        );
        private_write(&home.join("agent-session.json"), &serde_json::to_vec(self)?)
    }
    pub fn ensure_current(&self, home: &Path) -> Result<()> {
        let current = Self::load(home)?;
        ensure!(
            current.origin == self.origin
                && current.user_id == self.user_id
                && current.tenant_id == self.tenant_id
                && current.token == self.token,
            "customer account changed; synchronize again"
        );
        Ok(())
    }
}

pub struct Service {
    origin: reqwest::Url,
    client: reqwest::Client,
}
impl Service {
    pub fn new(origin: &str) -> Result<Self> {
        let url = reqwest::Url::parse(origin)?;
        let loopback = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && loopback),
            "agent service requires HTTPS (loopback HTTP only for tests)"
        );
        ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "agent service must be a credential-free origin"
        );
        Ok(Self {
            origin: url,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()?,
        })
    }
    fn url(&self, path: &str) -> Result<reqwest::Url> {
        Ok(self.origin.join(&format!("api/v1/{path}"))?)
    }
    async fn read(&self, mut response: reqwest::Response) -> Result<Value> {
        let status = response.status();
        // Never include service bodies or URLs with credentials in errors.
        ensure!(
            status.is_success(),
            "agent service request failed ({})",
            status.as_u16()
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|size| size <= MAX_RESPONSE_BYTES as u64),
            "agent response exceeds size limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_RESPONSE_BYTES,
                "agent response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes)?;
        ensure!(
            body.get("success").and_then(Value::as_bool) != Some(false),
            "agent service rejected request"
        );
        Ok(body.get("data").cloned().unwrap_or(body))
    }
    pub async fn index(&self) -> Result<Value> {
        self.read(
            self.client
                .get(self.url("public/agents/index")?)
                .send()
                .await?,
        )
        .await
    }
    pub async fn keys(&self) -> Result<BTreeMap<String, String>> {
        let body = self
            .read(
                self.client
                    .get(self.url("public/agents/keys")?)
                    .send()
                    .await?,
            )
            .await?;
        Ok(serde_json::from_value(
            body.get("keys")
                .cloned()
                .context("agent signing keys unavailable")?,
        )?)
    }
    pub async fn official(&self, id: &str, revision: &str, home: &Path) -> Result<AgentPackage> {
        ensure!(
            id.starts_with("official/") && valid_hash(revision),
            "official download requires fixed revision"
        );
        let keys = self.keys().await?;
        let data = self
            .read(
                self.client
                    .get(self.url("public/agents/package")?)
                    .query(&[("definition_id", id), ("revision", revision)])
                    .send()
                    .await?,
            )
            .await?;
        let bytes = serde_json::to_vec(&data)?;
        ensure!(
            bytes.len() <= MAX_PACKAGE_BYTES,
            "agent package exceeds size limit"
        );
        let package = AgentPackage::parse(&bytes)?;
        ensure!(
            package.sha256 == revision && package.definition.definition_id == id,
            "agent download identity mismatch"
        );
        package.verify_official(&keys)?;
        trusted_keys::cache(home, &self.origin, &keys)?;
        Ok(package)
    }
    pub async fn identify(&self, token: &str) -> Result<CustomerSession> {
        let body = self
            .read(
                self.client
                    .get(self.url("customer/agent-identity")?)
                    .bearer_auth(token)
                    .send()
                    .await?,
            )
            .await?;
        let field = |name| {
            body.get(name)
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .or_else(|| v.as_u64().map(|id| id.to_string()))
                })
                .context("customer identity unavailable")
        };
        Ok(CustomerSession {
            origin: self.origin.as_str().trim_end_matches('/').into(),
            token: token.into(),
            user_id: field("user_id")?,
            tenant_id: field("tenant_id")?,
        })
    }
    async fn verified_session(&self, session: &CustomerSession) -> Result<()> {
        let current = self.identify(&session.token).await?;
        ensure!(
            current.user_id == session.user_id
                && current.tenant_id == session.tenant_id
                && current.origin == session.origin,
            "customer account changed; login again"
        );
        Ok(())
    }
    pub async fn list(&self, session: &CustomerSession) -> Result<Vec<RemoteItem>> {
        self.verified_session(session).await?;
        self.list_items(session).await
    }
    async fn list_scoped(&self, home: &Path, session: &CustomerSession) -> Result<Vec<RemoteItem>> {
        session.ensure_current(home)?;
        self.verified_session(session).await?;
        session.ensure_current(home)?;
        let items = self.list_items(session).await?;
        session.ensure_current(home)?;
        Ok(items)
    }
    async fn list_items(&self, session: &CustomerSession) -> Result<Vec<RemoteItem>> {
        let body = self
            .read(
                self.client
                    .get(self.url("customer/agents")?)
                    .bearer_auth(&session.token)
                    .send()
                    .await?,
            )
            .await?;
        let items: Vec<RemoteItem> = serde_json::from_value(
            body.get("items")
                .cloned()
                .context("private agent list unavailable")?,
        )?;
        ensure!(items.len() <= 128, "private agent count exceeds limit");
        for item in &items {
            item.validate()?;
        }
        Ok(items)
    }
    pub async fn put(
        &self,
        session: &CustomerSession,
        package: &AgentPackage,
        etag: Option<&str>,
        request_id: &str,
    ) -> Result<RemoteItem> {
        let request = self
            .client
            .put(self.url("customer/agents")?)
            .query(&[("definition_id", &package.definition.definition_id)])
            .bearer_auth(&session.token)
            .header("Idempotency-Key", request_id)
            .json(package);
        let request = if let Some(etag) = etag {
            request.header("If-Match", etag)
        } else {
            request.header("If-None-Match", "*")
        };
        let data = self.read(request.send().await?).await?;
        let item: RemoteItem = serde_json::from_value(data)?;
        item.validate()?;
        ensure!(
            !item.deleted
                && item.sha256 == package.sha256
                && item.definition_id == package.definition.definition_id,
            "private upload response mismatch"
        );
        Ok(item)
    }
    pub async fn delete(
        &self,
        session: &CustomerSession,
        id: &str,
        etag: &str,
        request_id: &str,
    ) -> Result<RemoteItem> {
        let data = self
            .read(
                self.client
                    .delete(self.url("customer/agents")?)
                    .query(&[("definition_id", id)])
                    .bearer_auth(&session.token)
                    .header("If-Match", etag)
                    .header("Idempotency-Key", request_id)
                    .send()
                    .await?,
            )
            .await?;
        let item: RemoteItem = serde_json::from_value(data)?;
        item.validate()?;
        ensure!(
            item.deleted && item.definition_id == id,
            "private deletion response mismatch"
        );
        Ok(item)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RemoteItem {
    pub definition_id: String,
    pub sha256: String,
    pub version: String,
    pub deleted: bool,
    pub etag: String,
    pub package: Option<AgentPackage>,
}
impl RemoteItem {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.definition_id.starts_with("user/")
                && willdeep_core::agent_definition::valid_id(&self.definition_id)
                && valid_hash(&self.sha256)
                && !self.etag.is_empty(),
            "invalid private agent item"
        );
        if !self.deleted {
            let package = self.package.as_ref().context("private package missing")?;
            AgentPackage::parse(&serde_json::to_vec(package)?)?;
            ensure!(
                package.definition.definition_id == self.definition_id
                    && package.sha256 == self.sha256
                    && package.definition.version == self.version
                    && package.signature.is_none(),
                "private package identity mismatch"
            );
        } else {
            ensure!(
                self.package.is_none(),
                "deleted private agent must not contain a package"
            );
        }
        Ok(())
    }
}

#[derive(Default, Serialize)]
pub struct SyncReport {
    pub downloaded: Vec<String>,
    pub uploaded: Vec<String>,
    pub deleted: Vec<String>,
    pub conflicts: Vec<String>,
    pub conflict_details: Vec<Conflict>,
}

pub async fn synchronize(home: &Path) -> Result<SyncReport> {
    let session = CustomerSession::load(home)?;
    let service = Service::new(&session.origin)?;
    let remote = service.list_scoped(home, &session).await?;
    let library = session.library(home);
    let local = library.installations()?;
    let mut report = SyncReport::default();
    let remote: BTreeMap<_, _> = remote
        .into_iter()
        .map(|item| (item.definition_id.clone(), item))
        .collect();
    for (id, item) in &remote {
        session.ensure_current(home)?;
        let previous = local.get(id);
        if previous.is_some_and(|local| {
            local.dirty && local.remote_etag.as_deref() != Some(item.etag.as_str())
        }) {
            report.conflicts.push(id.clone());
            continue;
        }
        if item.deleted {
            if let Some(previous) = previous {
                if previous.pinned || previous.dirty {
                    report.conflicts.push(id.clone());
                } else {
                    library.remove(id, &previous.sha256)?;
                    report.deleted.push(id.clone());
                }
            }
        } else if previous.is_none_or(|local| !local.dirty && local.sha256 != item.sha256) {
            if previous.is_some_and(|local| local.pinned) {
                report.conflicts.push(id.clone());
                continue;
            }
            if library
                .install_synced(
                    item.package.as_ref().context("private package missing")?,
                    Origin::Private,
                    &BTreeMap::new(),
                    Some(item.etag.clone()),
                )
                .is_err()
            {
                report.conflicts.push(id.clone());
                continue;
            }
            super::disable_incompatible(
                &library,
                item.package.as_ref().context("private package missing")?,
            )?;
            report.downloaded.push(id.clone());
        }
    }
    for (id, item) in local.iter().filter(|(_, item)| item.dirty) {
        if report.conflicts.contains(id) {
            continue;
        }
        let package = library.package(&item.sha256)?;
        session.ensure_current(home)?;
        let key = stable_request_key(
            &session,
            "put",
            id,
            &item.sha256,
            item.remote_etag.as_deref(),
        );
        match service
            .put(&session, &package, item.remote_etag.as_deref(), &key)
            .await
        {
            Ok(result) => {
                session.ensure_current(home)?;
                library.mark_synced(id, &item.sha256, result.etag)?;
                report.uploaded.push(id.clone());
            }
            Err(error)
                if error.to_string().contains("(409)") || error.to_string().contains("(412)") =>
            {
                report.conflicts.push(id.clone())
            }
            Err(error) => return Err(error),
        }
    }
    for id in &report.conflicts {
        if let (Some(local), Some(remote)) = (local.get(id), remote.get(id)) {
            let mut conflict = Conflict::new(local, remote);
            conflict.local_version = library.package(&local.sha256)?.definition.version;
            report.conflict_details.push(conflict);
        }
    }
    Ok(report)
}

pub fn stable_request_key(
    session: &CustomerSession,
    operation: &str,
    id: &str,
    revision: &str,
    etag: Option<&str>,
) -> String {
    willdeep_core::agent_definition::digest(
        format!(
            "{}\n{}\n{}\n{operation}\n{id}\n{revision}\n{}",
            session.origin,
            session.user_id,
            session.tenant_id,
            etag.unwrap_or("*")
        )
        .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origins_do_not_allow_http_credentials_or_path_injection() {
        for origin in [
            "http://some.im",
            "https://user:secret@some.im",
            "https://some.im/path",
            "https://some.im?token=x",
        ] {
            assert!(Service::new(origin).is_err());
        }
        assert!(Service::new("http://127.0.0.1:3000").is_ok());
        assert!(Service::new(DEFAULT_ORIGIN).is_ok());
    }
    #[test]
    fn account_scope_and_idempotency_include_user_and_tenant() {
        let a = CustomerSession {
            origin: DEFAULT_ORIGIN.into(),
            token: "hidden".into(),
            user_id: "1".into(),
            tenant_id: "1".into(),
        };
        let mut b = a.clone();
        b.tenant_id = "2".into();
        assert_ne!(
            a.library(Path::new("/tmp/test")).root(),
            b.library(Path::new("/tmp/test")).root()
        );
        assert_ne!(
            stable_request_key(&a, "put", "user/a", "hash", None),
            stable_request_key(&b, "put", "user/a", "hash", None)
        );
    }
}

#[cfg(test)]
#[path = "service/tests.rs"]
mod integration_tests;
