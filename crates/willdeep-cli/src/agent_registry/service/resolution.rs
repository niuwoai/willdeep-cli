//! Explicit account-scoped conflict resolution; automatic sync never makes this choice.
use super::*;
use clap::ValueEnum;
use willdeep_core::agent_definition::store::Installation;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ResolveChoice {
    KeepLocal,
    UseCloud,
}

#[derive(Clone, Serialize)]
pub struct Conflict {
    pub definition_id: String,
    pub expected_local_revision: String,
    pub expected_remote_etag: String,
    pub remote_revision: String,
    pub remote_deleted: bool,
    pub local_version: String,
    pub remote_version: String,
    pub pinned: bool,
}
impl Conflict {
    pub(super) fn new(local: &Installation, remote: &RemoteItem) -> Self {
        Self {
            definition_id: local.definition_id.clone(),
            expected_local_revision: local.sha256.clone(),
            expected_remote_etag: remote.etag.clone(),
            remote_revision: remote.sha256.clone(),
            remote_deleted: remote.deleted,
            local_version: String::new(),
            remote_version: remote.version.clone(),
            pinned: local.pinned,
        }
    }
}

#[cfg(test)]
pub async fn resolve(
    home: &Path,
    id: &str,
    choice: ResolveChoice,
    expected_local_revision: &str,
    expected_remote_etag: &str,
    new_version: Option<&str>,
) -> Result<Value> {
    let session = CustomerSession::load(home)?;
    resolve_session(
        home,
        session,
        id,
        choice,
        expected_local_revision,
        expected_remote_etag,
        new_version,
    )
    .await
}

pub async fn resolve_scoped(
    home: &Path,
    id: &str,
    choice: ResolveChoice,
    expected_local_revision: &str,
    expected_remote_etag: &str,
    new_version: Option<&str>,
    expected_account: &Value,
) -> Result<Value> {
    let session = CustomerSession::load(home)?;
    let actual = serde_json::json!({"origin":session.origin,"user_id":session.user_id,"tenant_id":session.tenant_id});
    ensure!(
        *expected_account == actual,
        "customer account changed before queued resolution; synchronize again"
    );
    resolve_session(
        home,
        session,
        id,
        choice,
        expected_local_revision,
        expected_remote_etag,
        new_version,
    )
    .await
}

async fn resolve_session(
    home: &Path,
    session: CustomerSession,
    id: &str,
    choice: ResolveChoice,
    expected_local_revision: &str,
    expected_remote_etag: &str,
    new_version: Option<&str>,
) -> Result<Value> {
    ensure!(
        valid_hash(expected_local_revision) && !expected_remote_etag.is_empty(),
        "explicit local revision and remote ETag required"
    );
    let service = Service::new(&session.origin)?;
    let library = session.library(home);
    let local = library
        .installations()?
        .remove(id)
        .context("private agent not installed")?;
    ensure!(
        local.origin == Origin::Private && local.sha256 == expected_local_revision,
        "local revision changed; synchronize again"
    );
    let remote = service
        .list_scoped(home, &session)
        .await?
        .into_iter()
        .find(|item| item.definition_id == id)
        .context("cloud agent no longer exists; synchronize again")?;
    ensure!(
        remote.etag == expected_remote_etag,
        "cloud revision changed; synchronize again"
    );
    session.ensure_current(home)?;
    ensure!(
        library
            .installations()?
            .get(id)
            .is_some_and(|item| item.sha256 == expected_local_revision),
        "local revision changed; synchronize again"
    );
    if let Some(package) = &remote.package {
        library.archive_private(id, package)?;
    }
    let (resolved_revision, resolved_version) = match choice {
        ResolveChoice::KeepLocal => {
            let mut package = library.package(expected_local_revision)?;
            if let Some(version) = new_version {
                ensure!(
                    version != package.definition.version && version != remote.version,
                    "conflict resolution requires an unused new version"
                );
                ensure!(
                    !library
                        .history(id)?
                        .iter()
                        .any(|item| item.definition.version == version),
                    "conflict resolution version already exists in local history"
                );
                let mut definition = package.definition.clone();
                definition.version = version.into();
                package = AgentPackage::local(definition)?;
            } else {
                ensure!(
                    remote.version != package.definition.version || remote.sha256 == package.sha256,
                    "divergent content reused the same version; supply --new-version before keeping local"
                );
            }
            let key = stable_request_key(
                &session,
                "resolve-local",
                id,
                &package.sha256,
                Some(expected_remote_etag),
            );
            session.ensure_current(home)?;
            let result = service
                .put(&session, &package, Some(expected_remote_etag), &key)
                .await?;
            session.ensure_current(home)?;
            let compatible = super::super::compatibility(&package.definition).is_ok();
            library.resolve_private(
                id,
                expected_local_revision,
                Some(&package),
                &result.etag,
                compatible,
            )?;
            (package.sha256, package.definition.version)
        }
        ResolveChoice::UseCloud => {
            let latest = service
                .list_scoped(home, &session)
                .await?
                .into_iter()
                .find(|item| item.definition_id == id)
                .context("cloud agent no longer exists; synchronize again")?;
            ensure!(
                latest.etag == expected_remote_etag,
                "cloud revision changed; synchronize again"
            );
            session.ensure_current(home)?;
            let compatible = remote
                .package
                .as_ref()
                .is_none_or(|package| super::super::compatibility(&package.definition).is_ok());
            library.resolve_private(
                id,
                expected_local_revision,
                remote.package.as_ref(),
                &remote.etag,
                compatible,
            )?;
            (remote.sha256.clone(), remote.version.clone())
        }
    };
    Ok(
        serde_json::json!({"resolved":id,"choice":choice,"revision":resolved_revision,"version":resolved_version,"deleted":matches!(choice,ResolveChoice::UseCloud)&&remote.deleted}),
    )
}
