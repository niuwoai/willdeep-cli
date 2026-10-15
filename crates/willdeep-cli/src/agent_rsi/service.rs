//! Explicit opt-in summary upload. Inference keys are never accepted as customer identity.
use super::*;
use crate::agent_registry::service::{CustomerSession, Service};

struct Client {
    session: CustomerSession,
    client: reqwest::Client,
    home: PathBuf,
}
impl Client {
    async fn load(home: &Path) -> Result<Self> {
        let session = CustomerSession::load(home)?;
        let identified = Service::new(&session.origin)?
            .identify(&session.token)
            .await?;
        ensure!(
            identified.user_id == session.user_id
                && identified.tenant_id == session.tenant_id
                && identified.origin == session.origin,
            "customer account changed; login again"
        );
        let client = Self {
            session,
            home: home.into(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
        };
        client.ensure_current()?;
        Ok(client)
    }
    fn ensure_current(&self) -> Result<()> {
        let current = CustomerSession::load(&self.home)
            .context("customer session changed during RSI upload")?;
        ensure!(
            current.origin == self.session.origin
                && current.user_id == self.session.user_id
                && current.tenant_id == self.session.tenant_id
                && current.token == self.session.token,
            "customer session changed during RSI upload; operation cancelled"
        );
        Ok(())
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &[&str],
        document: Option<&Value>,
    ) -> Result<Value> {
        self.ensure_current()?;
        let mut url = reqwest::Url::parse(&self.session.origin)?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("invalid customer origin"))?;
            segments
                .clear()
                .extend(["api", "v1", "customer", "agent-rsi"]);
            segments.extend(path.iter().copied());
        }
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(&self.session.token);
        if let Some(document) = document {
            request = request
                .header(
                    "Idempotency-Key",
                    format!("rust-rsi-{}", contract::hash(document)?),
                )
                .body(contract::canonical(document)?)
                .header(reqwest::header::CONTENT_TYPE, "application/json");
        }
        let mut response = request
            .send()
            .await
            .context("RSI summary upload transport failed")?;
        ensure!(
            response.status().is_success(),
            "RSI service rejected request ({})",
            response.status().as_u16()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= contract::MAX_BYTES * 2,
                "RSI response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let result = contract::parse_json(&bytes)?;
        self.ensure_current()?;
        ensure!(result["success"] != false, "RSI service rejected request");
        Ok(result.get("data").cloned().unwrap_or(result))
    }
}
pub(super) async fn upload(home: &Path, id: &str, grant: &str) -> Result<Value> {
    ensure!(
        !grant.is_empty() && grant.len() <= 128,
        "summary upload requires an explicit grant ID"
    );
    let client = Client::load(home).await?;
    let (path, mut spec, mut candidate, _) = load(home, id)?;
    ensure!(
        path == account_storage(home, &client.session)
            .join("experiments")
            .join(digest(id.as_bytes())),
        "customer session changed while loading RSI assets"
    );
    client.ensure_current()?;
    let grant_record = client
        .request(reqwest::Method::GET, &["grants"], None)
        .await?;
    ensure!(
        grant_record["items"].as_array().is_some_and(|items| items
            .iter()
            .any(|item| item["document"]["grant_id"] == grant && item["revoked"] != true)),
        "explicit active evidence grant not found"
    );
    candidate["grant_id"] = grant.into();
    spec["grant_id"] = grant.into();
    client
        .request(reqwest::Method::POST, &["candidates"], Some(&candidate))
        .await?;
    client
        .request(reqwest::Method::POST, &["experiments"], Some(&spec))
        .await?;
    let mut rows = runner::evidence(&path)?;
    sort_evidence(&mut rows);
    let mut receipts = Vec::new();
    for row in rows {
        contract::bind_evidence(&spec, &row)?;
        let admission = contract::parse_json(&std::fs::read(
            path.join("trials")
                .join(row["trial_id"].as_str().unwrap())
                .join("admission.json"),
        )?)?;
        client
            .request(
                reqwest::Method::POST,
                &["experiments", id, "admissions"],
                Some(&admission),
            )
            .await?;
        let receipt = client
            .request(
                reqwest::Method::POST,
                &["experiments", id, "evidence"],
                Some(&row),
            )
            .await?;
        let mut expected = row.clone();
        expected["trust"] = "client_reported".into();
        ensure!(
            receipt["document"] == expected && receipt["sha256"] == contract::hash(&expected)?,
            "service evidence receipt mismatch"
        );
        receipts.push(receipt);
    }
    let value = json!({"experiment_id":id,"grant_id":grant,"uploaded_evidence":receipts.len(),"receipts":receipts,"activated":false,"trust":"client_reported"});
    private_write(
        &path.join(format!("upload-{}.json", digest(grant.as_bytes()))),
        &serde_json::to_vec_pretty(&value)?,
    )?;
    Ok(value)
}
fn sort_evidence(rows: &mut [Value]) {
    rows.sort_by_key(|row| {
        (
            match row["split"].as_str() {
                Some("validation") => 0,
                Some("regression") => 1,
                _ => 2,
            },
            row["task_id"].as_str().unwrap_or("").to_owned(),
            row["arm"].as_str().unwrap_or("").to_owned(),
            row["repetition"].as_u64().unwrap_or(0),
            row["attempt"].as_u64().unwrap_or(0),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[test]
    fn upload_order_respects_stage_and_resume_parent_before_child() {
        let mut rows = vec![
            json!({"split":"holdout","task_id":"holdout-1","arm":"baseline","repetition":1,"attempt":1}),
            json!({"split":"validation","task_id":"validation-1","arm":"baseline","repetition":1,"attempt":2}),
            json!({"split":"regression","task_id":"regression-1","arm":"baseline","repetition":1,"attempt":1}),
            json!({"split":"validation","task_id":"validation-1","arm":"baseline","repetition":1,"attempt":1}),
        ];
        sort_evidence(&mut rows);
        assert_eq!(rows[0]["attempt"], 1);
        assert_eq!(rows[1]["attempt"], 2);
        assert_eq!(rows[2]["split"], "regression");
        assert_eq!(rows[3]["split"], "holdout");
    }
    #[derive(Clone)]
    struct State {
        home: PathBuf,
        session: CustomerSession,
        switch_identity: bool,
        posts: Arc<AtomicUsize>,
    }
    fn switch(state: &State) {
        CustomerSession {
            user_id: "other-user".into(),
            token: "test-other-session".into(),
            ..state.session.clone()
        }
        .save(&state.home)
        .unwrap();
    }
    async fn identity(
        axum::extract::State(state): axum::extract::State<State>,
    ) -> axum::Json<Value> {
        if state.switch_identity {
            switch(&state);
        }
        axum::Json(
            json!({"success":true,"data":{"user_id":state.session.user_id,"tenant_id":state.session.tenant_id}}),
        )
    }
    async fn grants(axum::extract::State(state): axum::extract::State<State>) -> axum::Json<Value> {
        switch(&state);
        axum::Json(
            json!({"success":true,"data":{"items":[{"document":{"grant_id":"race-grant"}}]}}),
        )
    }
    async fn mutation(
        axum::extract::State(state): axum::extract::State<State>,
    ) -> axum::Json<Value> {
        state.posts.fetch_add(1, Ordering::SeqCst);
        axum::Json(json!({"success":true,"data":{}}))
    }
    async fn check_switch(switch_identity: bool) {
        let home = std::env::temp_dir().join(format!("rsi-account-race-{}", uuid::Uuid::new_v4()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let session = CustomerSession {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            token: "test-original-session".into(),
            user_id: "original-user".into(),
            tenant_id: "original-tenant".into(),
        };
        session.save(&home).unwrap();
        let values = contract::golden();
        let baseline = package(&values["values"]["candidate"]["baseline_package"]).unwrap();
        let candidate = package(&values["values"]["candidate"]["candidate_package"]).unwrap();
        prepare(&home, "account-race", baseline, candidate, Suite::builtin()).unwrap();
        let posts = Arc::new(AtomicUsize::new(0));
        let state = State {
            home: home.clone(),
            session,
            switch_identity,
            posts: posts.clone(),
        };
        let app = axum::Router::new()
            .route(
                "/api/v1/customer/agent-identity",
                axum::routing::get(identity),
            )
            .route(
                "/api/v1/customer/agent-rsi/grants",
                axum::routing::get(grants),
            )
            .route(
                "/api/v1/customer/agent-rsi/candidates",
                axum::routing::post(mutation),
            )
            .with_state(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let result = upload(&home, "account-race", "race-grant").await;
        assert!(result.unwrap_err().to_string().contains("session changed"));
        assert_eq!(posts.load(Ordering::SeqCst), 0);
        server.abort();
        std::fs::remove_dir_all(home).unwrap();
    }
    #[tokio::test]
    async fn switching_during_identity_does_not_upload_old_assets_to_new_account() {
        check_switch(true).await;
    }
    #[tokio::test]
    async fn switching_after_asset_load_cancels_before_first_mutation() {
        check_switch(false).await;
    }
}
