//! 带签名的真实回环 HTTP 测试：每台实例有独立 home、联系人表和会话。
use super::*;
use serde_json::json;

struct Peer {
    home: PathBuf,
    identity: crate::identity::Identity,
    url: String,
    server: tokio::task::JoinHandle<()>,
}

async fn peer(root: &Path, name: &str) -> Peer {
    let home = root.join(name);
    std::fs::create_dir_all(&home).unwrap();
    let identity = crate::identity::rename(&home, name).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(IntakeState {
        service: Arc::new(IntakeService::new(&home)),
        settings: IntakeSettings::default(),
        federation: FederationSettings {
            enabled: true,
            workspace: home.display().to_string(),
            reply: "always".to_owned(),
            max_auto_turns: federation::DEFAULT_MAX_AUTO_TURNS,
        },
        form_signing_key: [0; 32],
        consumed_form_nonces: std::sync::Mutex::new(HashMap::new()),
    });
    let app = Router::new()
        .route("/p/v1/message", post(federation_message))
        .with_state(state);
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Peer {
        home,
        identity,
        url,
        server,
    }
}

fn pair(left: &Peer, right: &Peer, secret: &str) {
    federation::upsert_contact(
        &left.home,
        Contact {
            id: right.identity.id,
            name: right.identity.name.clone(),
            url: right.url.clone(),
            secret: secret.to_owned(),
            session_id: None,
            added_at: federation::now_seconds(),
            last_seen: None,
            auto_turns: 0,
        },
    )
    .unwrap();
}

fn new_session(peer: &Peer) -> uuid::Uuid {
    let store = willdeep_core::SessionStore::new(&peer.home);
    let mut session = willdeep_core::Session::new(peer.home.clone(), None, "联邦群聊");
    store.save(&mut session).unwrap();
    session.id
}

async fn signed(from: &Peer, to: &Peer, body: serde_json::Value, secret: &str) -> StatusCode {
    let bytes = serde_json::to_vec(&body).unwrap();
    let from_id = from.identity.id.to_string();
    let to_id = to.identity.id.to_string();
    let nonce = uuid::Uuid::new_v4().to_string();
    let message_id = uuid::Uuid::new_v4().to_string();
    let ts = federation::now_seconds();
    let signature = federation::sign(
        secret,
        &federation::Signed {
            from: &from_id,
            to: &to_id,
            ts,
            nonce: &nonce,
            message_id: &message_id,
        },
        &bytes,
    )
    .unwrap();
    let result = reqwest::Client::new()
        .post(format!("{}/p/v1/message", to.url))
        .header("x-willdeep-from", from_id)
        .header("x-willdeep-to", to_id)
        .header("x-willdeep-msg", message_id)
        .header("x-willdeep-ts", ts.to_string())
        .header("x-willdeep-nonce", nonce)
        .header("x-willdeep-signature", format!("sha256={signature}"))
        .body(bytes)
        .send()
        .await
        .unwrap();
    StatusCode::from_u16(result.status().as_u16()).unwrap()
}

#[tokio::test]
async fn three_signed_peers_enforce_group_origin_pause_and_notices() {
    let root = std::env::temp_dir().join(format!("willdeep-group-http-{}", uuid::Uuid::new_v4()));
    let host = peer(&root, "host").await;
    let beta = peer(&root, "beta").await;
    let gamma = peer(&root, "gamma").await;
    let outsider = peer(&root, "outsider").await;
    let key_beta = federation::new_secret();
    let key_gamma = federation::new_secret();
    let key_outsider = federation::new_secret();
    for (other, key) in [
        (&beta, &key_beta),
        (&gamma, &key_gamma),
        (&outsider, &key_outsider),
    ] {
        pair(&host, other, key);
        pair(other, &host, key);
    }
    let group =
        federation::create_group(&host.home, "team", &["beta".into(), "gamma".into()]).unwrap();
    let host_session = new_session(&host);
    federation::note_group_session(&host.home, group.id, host_session).unwrap();
    for member in [&beta, &gamma] {
        let session = new_session(member);
        federation::note_joined_group(
            &member.home,
            host.identity.id,
            group.id,
            &group.name,
            &group.members,
            group.generation,
            false,
        )
        .unwrap();
        federation::note_joined_session(&member.home, group.id, session).unwrap();
    }
    let forged = |origin: uuid::Uuid, generation: u64| {
        json!({
            "protocol": federation::PROTOCOL, "to": host.identity.id,
            "text": "@host fake", "group_id": group.id,
            "origin": origin, "from_agent": true, "generation": generation,
            "mention_depth": 1,
        })
    };
    assert_eq!(
        signed(
            &outsider,
            &host,
            forged(outsider.identity.id, group.generation),
            &key_outsider
        )
        .await,
        StatusCode::FORBIDDEN,
        "a paired outsider is not a member"
    );
    assert_eq!(
        signed(
            &beta,
            &host,
            forged(gamma.identity.id, group.generation),
            &key_beta
        )
        .await,
        StatusCode::FORBIDDEN,
        "a member cannot forge another member's origin"
    );
    assert_eq!(
        signed(
            &beta,
            &host,
            forged(beta.identity.id, group.generation),
            &key_gamma
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "the wrong shared key cannot authenticate the member"
    );

    let suppressed = json!({
        "protocol": federation::PROTOCOL, "to": beta.identity.id,
        "text": "@beta host quota exhausted", "group_id": group.id,
        "group_name": group.name, "group_members": group.members,
        "generation": group.generation, "mention_limit": 3,
        "auto_reply_allowed": false,
        "origin": host.identity.id, "from_agent": true,
    });
    assert_eq!(
        signed(&host, &beta, suppressed, &key_beta).await,
        StatusCode::ACCEPTED,
        "a signed host quota denial records text without submitting a model turn"
    );
    let beta_session = federation::load(&beta.home).unwrap().joined_groups[0]
        .session_id
        .unwrap();
    let beta_messages = willdeep_core::SessionStore::new(&beta.home)
        .load(beta_session)
        .unwrap()
        .messages;
    assert_eq!(
        beta_messages.last().unwrap().role,
        willdeep_core::Role::System
    );
    assert!(
        beta_messages
            .last()
            .unwrap()
            .content
            .contains("quota exhausted")
    );
    assert!(
        !beta.home.join("runtime/turns.json").exists(),
        "host denied the auto reply, so no Runtime turn is queued"
    );

    let paused = federation::pause_group(&host.home, "team").unwrap();
    assert_eq!(
        signed(
            &beta,
            &host,
            forged(beta.identity.id, group.generation),
            &key_beta
        )
        .await,
        StatusCode::FORBIDDEN,
        "an in-flight reply from the old generation is rejected"
    );
    let before = willdeep_core::SessionStore::new(&host.home)
        .load(host_session)
        .unwrap()
        .messages
        .len();
    assert_eq!(
        signed(
            &beta,
            &host,
            forged(beta.identity.id, paused.generation),
            &key_beta
        )
        .await,
        StatusCode::ACCEPTED,
        "a paused group records the message without starting a model turn"
    );
    let after = willdeep_core::SessionStore::new(&host.home)
        .load(host_session)
        .unwrap()
        .messages
        .len();
    assert_eq!(
        after,
        before + 1,
        "only a system line was recorded while paused"
    );

    let notice = federation::mention_limit_notice(3);
    forward_group_note(&host.home, &host.identity, &paused, &notice).await;
    for member in [&beta, &gamma] {
        let joined = federation::load(&member.home)
            .unwrap()
            .joined_groups
            .remove(0);
        assert!(joined.paused, "pause reached the member");
        let session = willdeep_core::SessionStore::new(&member.home)
            .load(joined.session_id.unwrap())
            .unwrap();
        assert_eq!(
            session.messages.last().unwrap().role,
            willdeep_core::Role::System
        );
        assert_eq!(
            session.messages.last().unwrap().content,
            notice,
            "the limit explanation reaches every member, including the speaker"
        );
    }
    // 这个夹具只验签名与群状态，不应该有 Runtime。若单测进程被当成
    // daemon 拉起，`start()` 在 spawn 前就会建出 runtime/ 和 daemon.lock；
    // 断言这些痕迹不存在，防止以后换条路径又把 libtest harness 拉起来。
    for peer in [&host, &beta, &gamma, &outsider] {
        assert!(
            !peer.home.join("runtime/daemon.json").exists()
                && !peer.home.join("runtime/daemon.lock").exists(),
            "the signed HTTP fixture must not start a Runtime daemon from the test harness"
        );
    }
    for member in [host, beta, gamma, outsider] {
        member.server.abort();
    }
    std::fs::remove_dir_all(root).unwrap();
}
