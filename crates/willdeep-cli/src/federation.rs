//! 实例联邦 P1：两台公网可达的 willdeep 互发消息。
//!
//! 身份来自 [`crate::identity`]（UUID 是身份，名字只是标签）。联系人表存在
//! `$WILLDEEP_HOME/federation.json`（0600），每对联系人一个 32 字节共享密钥，
//! 请求按「时间戳 + nonce + 正文摘要」做 HMAC。地址必须是 https——回环地址
//! 例外，方便本机双实例对拍。
//!
//! 这一层只管「怎么把一句话安全地送到对面」，不管会话：入站落到哪条会话、
//! 回复怎么发出去，在 `daemon/intake.rs` 里接。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FEDERATION_SCHEMA: &str = "willdeep.federation.v1";
pub const PROTOCOL: &str = "willdeep.federation.v1";
/// 单条消息的字符上限。和分享访客同一量级：够说清一件事，不够塞一篇稿。
pub const MAX_MESSAGE_CHARS: usize = 8 * 1024;
/// 时间戳允许的偏移。超过就当重放或时钟漂移，直接拒绝。
pub const CLOCK_SKEW_SECONDS: u64 = 300;
/// 每个联系人每分钟最多接多少条。和分享访客的 20 条/分钟对齐。
pub const MESSAGES_PER_MINUTE: u32 = 20;
pub const DEFAULT_MAX_AUTO_TURNS: u32 = 6;
pub const INVITE_TTL_SECONDS: u64 = 10 * 60;

type SignatureMac = Hmac<Sha256>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Contact {
    /// 对端的实例 UUID。名字会变，它不会。
    pub id: uuid::Uuid,
    /// 对端最后一次告诉我们的名字。只用于显示。
    pub name: String,
    /// 对端入口的基地址，比如 `https://alex.example.com`，不带路径。
    pub url: String,
    /// 十六进制 32 字节共享密钥。只在两边各存一份，不走配置文件。
    pub secret: String,
    /// 这段联邦对话落在本机的哪条会话上。
    #[serde(default)]
    pub session_id: Option<uuid::Uuid>,
    pub added_at: u64,
    #[serde(default)]
    pub last_seen: Option<u64>,
    /// 自上次人工发言以来已经自动回了多少轮。到上限就停，等人工接手。
    #[serde(default)]
    pub auto_turns: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invite {
    /// 只存令牌的 SHA-256：邀请文件泄漏时也不能直接拿来配对。
    pub token_hash: String,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationStore {
    pub schema: String,
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub invites: Vec<Invite>,
}

impl Default for FederationStore {
    fn default() -> Self {
        Self {
            schema: FEDERATION_SCHEMA.to_owned(),
            contacts: Vec::new(),
            invites: Vec::new(),
        }
    }
}

pub fn path(home: &Path) -> PathBuf {
    home.join("federation.json")
}

/// 读联系人表。文件不存在就是空表——联邦没配过不是错误。
pub fn load(home: &Path) -> Result<FederationStore> {
    match std::fs::read(path(home)) {
        Ok(bytes) => {
            let store: FederationStore =
                serde_json::from_slice(&bytes).context("federation.json is not valid")?;
            if store.schema != FEDERATION_SCHEMA {
                bail!(
                    "federation.json uses schema {}, expected {FEDERATION_SCHEMA}",
                    store.schema
                );
            }
            Ok(store)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FederationStore::default()),
        Err(error) => Err(error).context("cannot read federation.json"),
    }
}

pub fn save(home: &Path, store: &FederationStore) -> Result<()> {
    std::fs::create_dir_all(home).context("create the willdeep home directory")?;
    let temporary = home.join(format!(".federation-{}.json", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(store).context("encode federation contacts")?;
    std::fs::write(&temporary, bytes).context("write federation contacts")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .context("restrict federation file permissions")?;
    }
    std::fs::rename(&temporary, path(home)).context("replace federation.json")?;
    Ok(())
}

/// 按名字或 UUID 找联系人。名字大小写不敏感；重名时要求用 UUID。
pub fn find<'a>(store: &'a FederationStore, query: &str) -> Result<&'a Contact> {
    let query = query.trim();
    if let Ok(id) = uuid::Uuid::parse_str(query)
        && let Some(contact) = store.contacts.iter().find(|contact| contact.id == id)
    {
        return Ok(contact);
    }
    let matches: Vec<&Contact> = store
        .contacts
        .iter()
        .filter(|contact| contact.name.eq_ignore_ascii_case(query))
        .collect();
    match matches.as_slice() {
        [contact] => Ok(contact),
        [] => bail!("no federation contact named `{query}`"),
        _ => bail!("more than one contact is named `{query}`; use the UUID instead"),
    }
}

pub fn upsert_contact(home: &Path, contact: Contact) -> Result<Contact> {
    let mut store = load(home)?;
    match store
        .contacts
        .iter_mut()
        .find(|existing| existing.id == contact.id)
    {
        Some(existing) => {
            // 重新配对会换新的共享密钥；名字和地址跟着对端最新的说法走。
            existing.name = contact.name;
            existing.url = contact.url;
            existing.secret = contact.secret;
            let updated = existing.clone();
            save(home, &store)?;
            Ok(updated)
        }
        None => {
            store.contacts.push(contact.clone());
            save(home, &store)?;
            Ok(contact)
        }
    }
}

pub fn remove_contact(home: &Path, query: &str) -> Result<Contact> {
    let mut store = load(home)?;
    let contact = find(&store, query)?.clone();
    store.contacts.retain(|item| item.id != contact.id);
    save(home, &store)?;
    Ok(contact)
}

/// 手动发言会重置自动回复的计数：人接手了，循环风险就清零。
pub fn note_manual_message(home: &Path, id: uuid::Uuid) -> Result<()> {
    let mut store = load(home)?;
    if let Some(contact) = store.contacts.iter_mut().find(|item| item.id == id) {
        contact.auto_turns = 0;
        save(home, &store)?;
    }
    Ok(())
}

pub fn note_auto_reply(home: &Path, id: uuid::Uuid) -> Result<u32> {
    let mut store = load(home)?;
    let Some(contact) = store.contacts.iter_mut().find(|item| item.id == id) else {
        return Ok(0);
    };
    contact.auto_turns = contact.auto_turns.saturating_add(1);
    let count = contact.auto_turns;
    save(home, &store)?;
    Ok(count)
}

pub fn note_session(home: &Path, id: uuid::Uuid, session_id: uuid::Uuid) -> Result<()> {
    let mut store = load(home)?;
    if let Some(contact) = store.contacts.iter_mut().find(|item| item.id == id) {
        contact.session_id = Some(session_id);
        contact.last_seen = Some(now_seconds());
        save(home, &store)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 邀请

pub struct InviteCreated {
    pub token: String,
    pub expires_at: u64,
}

/// 生成一个一次性邀请。过期的邀请顺手清掉，文件不会越攒越多。
pub fn create_invite(home: &Path) -> Result<InviteCreated> {
    let mut store = load(home)?;
    let now = now_seconds();
    store.invites.retain(|invite| invite.expires_at > now);
    let token = new_token();
    let expires_at = now + INVITE_TTL_SECONDS;
    store.invites.push(Invite {
        token_hash: hash_token(&token),
        created_at: now,
        expires_at,
    });
    save(home, &store)?;
    Ok(InviteCreated { token, expires_at })
}

/// 消费一个邀请。令牌正确、未过期、且只能用一次。
pub fn consume_invite(home: &Path, token: &str) -> Result<()> {
    let mut store = load(home)?;
    let now = now_seconds();
    let hash = hash_token(token.trim());
    let Some(index) = store
        .invites
        .iter()
        .position(|invite| invite.token_hash == hash && invite.expires_at > now)
    else {
        bail!("the invitation is unknown or has expired");
    };
    store.invites.remove(index);
    save(home, &store)?;
    Ok(())
}

// ---------------------------------------------------------------- 签名

fn canonical(from: &str, to: &str, ts: u64, nonce: &str, message_id: &str, body: &[u8]) -> String {
    let digest = hex_encode(&Sha256::digest(body));
    format!("{PROTOCOL}\n{from}\n{to}\n{ts}\n{nonce}\n{message_id}\n{digest}")
}

pub fn sign(secret: &str, from: &str, to: &str, ts: u64, nonce: &str, message_id: &str, body: &[u8]) -> Result<String> {
    let mut mac = SignatureMac::new_from_slice(secret.as_bytes())
        .map_err(|error| anyhow::anyhow!("invalid federation secret: {error}"))?;
    mac.update(canonical(from, to, ts, nonce, message_id, body).as_bytes());
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

pub fn verify(
    secret: &str,
    from: &str,
    to: &str,
    ts: u64,
    nonce: &str,
    message_id: &str,
    body: &[u8],
    signature: &str,
) -> Result<()> {
    let mut mac = SignatureMac::new_from_slice(secret.as_bytes())
        .map_err(|error| anyhow::anyhow!("invalid federation secret: {error}"))?;
    mac.update(canonical(from, to, ts, nonce, message_id, body).as_bytes());
    let Ok(expected) = hex_decode(signature) else {
        bail!("the signature is not hexadecimal");
    };
    mac.verify_slice(&expected)
        .map_err(|_| anyhow::anyhow!("the signature does not match"))
}

/// 时间戳必须在允许的偏移内。太大或太小都可能是重放，也可能是时钟没对。
pub fn check_timestamp(ts: u64, now: u64) -> Result<()> {
    let drift = now.abs_diff(ts);
    if drift > CLOCK_SKEW_SECONDS {
        bail!("the message timestamp is off by {drift}s");
    }
    Ok(())
}

// ---------------------------------------------------------------- 网络

#[derive(Serialize)]
struct Outgoing<'a> {
    protocol: &'a str,
    to: String,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<&'a str>,
    client_message_id: &'a str,
}

/// 把一句话发给对端。返回 202 即已进入对端的会话队列，不代表已经回复。
pub async fn send_message_as(
    from: uuid::Uuid,
    contact: &Contact,
    text: &str,
) -> Result<()> {
    let text = text.trim();
    if text.is_empty() {
        bail!("the message is empty");
    }
    if text.chars().count() > MAX_MESSAGE_CHARS {
        bail!("the message exceeds {MAX_MESSAGE_CHARS} characters");
    }
    let to = contact.id.to_string();
    let message_id = uuid::Uuid::new_v4().to_string();
    let nonce = new_token();
    let ts = now_seconds();
    let body = serde_json::to_vec(&Outgoing {
        protocol: PROTOCOL,
        to: to.clone(),
        text,
        reply_to: None,
        client_message_id: &message_id,
    })
    .context("encode the federation message")?;
    let from = from.to_string();
    let signature = sign(&contact.secret, &from, &to, ts, &nonce, &message_id, &body)?;
    let response = client()?
        .post(format!("{}/p/v1/message", contact.url.trim_end_matches('/')))
        .header("content-type", "application/json")
        .header("x-willdeep-from", &from)
        .header("x-willdeep-to", &to)
        .header("x-willdeep-msg", &message_id)
        .header("x-willdeep-ts", ts.to_string())
        .header("x-willdeep-nonce", &nonce)
        .header("x-willdeep-signature", format!("sha256={signature}"))
        .body(body)
        .send()
        .await
        .with_context(|| format!("cannot reach {}", contact.url))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let detail = response.text().await.unwrap_or_default();
    bail!(
        "{} rejected the message ({status}): {}",
        contact.name,
        detail.chars().take(300).collect::<String>()
    );
}

#[derive(Deserialize)]
struct JoinResponse {
    protocol: String,
    id: uuid::Uuid,
    name: String,
    secret: String,
}

/// 用别人的邀请 URL 加入。`our_url` 是对端回连我们时用的基地址。
pub async fn join(invite_url: &str, identity: &crate::identity::Identity, our_url: &str) -> Result<Contact> {
    let invite = parse_invite_url(invite_url)?;
    let base = invite.base;
    let body = serde_json::to_vec(&serde_json::json!({
        "protocol": PROTOCOL,
        "id": identity.id.to_string(),
        "name": identity.name,
        "url": our_url.trim_end_matches('/'),
    }))
    .context("encode the join request")?;
    let response = client()?
        .post(format!("{base}/p/v1/join/{}", invite.token))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .with_context(|| format!("cannot reach {base}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "{base} refused the invitation ({status}): {}",
            text.chars().take(300).collect::<String>()
        );
    }
    let reply: JoinResponse = serde_json::from_str(&text).context("decode the join response")?;
    if reply.protocol != PROTOCOL {
        bail!("{base} speaks {}, not {PROTOCOL}", reply.protocol);
    }
    crate::identity::validate_name(&reply.name)
        .context("the peer sent an invalid instance name")?;
    if hex_decode(&reply.secret).map(|bytes| bytes.len()).unwrap_or(0) != 32 {
        bail!("{base} sent a malformed shared secret");
    }
    Ok(Contact {
        id: reply.id,
        name: reply.name,
        url: base,
        secret: reply.secret,
        session_id: None,
        added_at: now_seconds(),
        last_seen: None,
        auto_turns: 0,
    })
}

struct ParsedInvite {
    base: String,
    token: String,
}

/// `https://alex.example.com/p/v1/join/<token>` →（`https://alex.example.com`, token）。
/// 只认这一种形状：邀请本来就是我们自己打印的，别的写法没有兼容价值。
fn parse_invite_url(value: &str) -> Result<ParsedInvite> {
    let parsed = reqwest::Url::parse(value.trim())
        .with_context(|| format!("`{}` is not a valid invitation URL", value.trim()))?;
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|segments| segments.collect())
        .unwrap_or_default();
    let [prefix, version, join, token] = segments.as_slice() else {
        bail!("an invitation URL looks like https://host/p/v1/join/<token>");
    };
    if *prefix != "p" || *version != "v1" || *join != "join" || token.is_empty() {
        bail!("an invitation URL looks like https://host/p/v1/join/<token>");
    }
    let base = format!(
        "{}://{}{}",
        parsed.scheme(),
        parsed
            .host_str()
            .context("the invitation URL needs a host")?,
        parsed
            .port()
            .map(|port| format!(":{port}"))
            .unwrap_or_default()
    );
    let base = validate_peer_url(&base)?;
    Ok(ParsedInvite {
        base,
        token: (*token).to_owned(),
    })
}

pub fn validate_peer_url(value: &str) -> Result<String> {
    let trimmed = value.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(trimmed)
        .with_context(|| format!("`{trimmed}` is not a valid URL"))?;
    if parsed.scheme() != "https" && !is_loopback(&parsed) {
        bail!("a peer address must use https (http is allowed only for loopback)");
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        bail!("the peer address needs a host");
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("a peer address is an origin without a path, query or fragment");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("a peer address must not carry credentials");
    }
    Ok(trimmed.to_owned())
}

fn is_loopback(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || host.starts_with("127.")
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(format!("willdeep/{}", willdeep_core::VERSION))
        .build()
        .context("build the federation HTTP client")
}

// ---------------------------------------------------------------- 杂项

/// 邀请链接。`public_url` 必须已经配好——回环地址拼出来的链接只能自己打开。
pub fn invite_url(public_url: &str, token: &str) -> String {
    format!(
        "{}/p/v1/join/{token}",
        public_url.trim().trim_end_matches('/')
    )
}

pub fn new_token() -> String {    // 两个 v4 拼起来是 256 位随机量；邀请令牌和 nonce 都用它。
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub fn new_secret() -> String {
    new_token()
}

fn hash_token(token: &str) -> String {
    hex_encode(&Sha256::digest(token.as_bytes()))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(value: &str) -> Result<Vec<u8>> {
    let value = value.trim();
    if value.len() % 2 != 0 {
        bail!("hex string has an odd length");
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| anyhow::anyhow!("hex string contains a non-hex character"))
        })
        .collect()
}

pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        let root = std::env::temp_dir().join(format!("willdeep-fed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn contact() -> Contact {
        Contact {
            id: uuid::Uuid::new_v4(),
            name: "alex".to_owned(),
            url: "https://alex.example.com".to_owned(),
            secret: new_secret(),
            session_id: None,
            added_at: now_seconds(),
            last_seen: None,
            auto_turns: 0,
        }
    }

    #[test]
    fn a_missing_store_is_empty_and_a_saved_one_round_trips() {
        let root = home();
        assert!(load(&root).unwrap().contacts.is_empty());
        let contact = contact();
        upsert_contact(&root, contact.clone()).unwrap();
        let store = load(&root).unwrap();
        assert_eq!(store.contacts, vec![contact.clone()]);
        // 同一个 UUID 再存一次是更新，不是追加。
        let mut renamed = contact.clone();
        renamed.name = "alex2".to_owned();
        upsert_contact(&root, renamed.clone()).unwrap();
        assert_eq!(load(&root).unwrap().contacts, vec![renamed]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_invitation_can_be_consumed_once() {
        let root = home();
        let created = create_invite(&root).unwrap();
        consume_invite(&root, &created.token).unwrap();
        assert!(consume_invite(&root, &created.token).is_err(), "single use");
        assert!(consume_invite(&root, "nope").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_expired_invitation_is_refused() {
        let root = home();
        let created = create_invite(&root).unwrap();
        let mut store = load(&root).unwrap();
        store.invites[0].expires_at = now_seconds() - 1;
        save(&root, &store).unwrap();
        assert!(consume_invite(&root, &created.token).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_signature_covers_every_part_of_the_request() {
        let secret = new_secret();
        let body = br#"{"text":"hi"}"#;
        let signature = sign(&secret, "a", "b", 1000, "n", "m", body).unwrap();
        verify(&secret, "a", "b", 1000, "n", "m", body, &signature).unwrap();
        for changed in [
            sign(&secret, "x", "b", 1000, "n", "m", body).unwrap(),
            sign(&secret, "a", "x", 1000, "n", "m", body).unwrap(),
            sign(&secret, "a", "b", 1001, "n", "m", body).unwrap(),
            sign(&secret, "a", "b", 1000, "x", "m", body).unwrap(),
            sign(&secret, "a", "b", 1000, "n", "x", body).unwrap(),
            sign(&secret, "a", "b", 1000, "n", "m", b"{}").unwrap(),
        ] {
            assert!(verify(&secret, "a", "b", 1000, "n", "m", body, &changed).is_err());
        }
        assert!(verify("other", "a", "b", 1000, "n", "m", body, &signature).is_err());
    }

    #[test]
    fn the_timestamp_window_is_two_sided() {
        assert!(check_timestamp(1000, 1000).is_ok());
        assert!(check_timestamp(1000, 1000 + CLOCK_SKEW_SECONDS).is_ok());
        assert!(check_timestamp(1000, 1001 + CLOCK_SKEW_SECONDS).is_err());
        assert!(check_timestamp(1000, 1000 - CLOCK_SKEW_SECONDS).is_ok());
        assert!(check_timestamp(1000, 999 - CLOCK_SKEW_SECONDS).is_err());
    }

    #[test]
    fn a_peer_address_must_be_a_bare_origin_over_https() {
        assert_eq!(
            validate_peer_url("https://alex.example.com/").unwrap(),
            "https://alex.example.com"
        );
        assert!(validate_peer_url("http://127.0.0.1:9857").is_ok(), "loopback may use http");
        assert!(validate_peer_url("http://alex.example.com").is_err());
        assert!(validate_peer_url("https://alex.example.com/p").is_err());
        assert!(validate_peer_url("https://user:secret@alex.example.com").is_err());
        assert!(validate_peer_url("alex.example.com").is_err());
    }

    #[test]
    fn contacts_are_found_by_name_or_uuid_and_ambiguous_names_are_refused() {
        let root = home();
        let first = contact();
        upsert_contact(&root, first.clone()).unwrap();
        let mut second = contact();
        second.name = first.name.clone();
        upsert_contact(&root, second.clone()).unwrap();
        let store = load(&root).unwrap();
        assert!(find(&store, "alex").is_err(), "two contacts share the name");
        assert_eq!(find(&store, &first.id.to_string()).unwrap().id, first.id);
        assert!(
            find(&store, "ALEX")
                .unwrap_err()
                .to_string()
                .contains("more than one")
        );
        remove_contact(&root, &first.id.to_string()).unwrap();
        assert_eq!(find(&load(&root).unwrap(), "alex").unwrap().id, second.id);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_invitation_url_is_an_origin_plus_the_join_path() {
        let parsed = parse_invite_url("https://alex.example.com/p/v1/join/abc123").unwrap();
        assert_eq!(parsed.base, "https://alex.example.com");
        assert_eq!(parsed.token, "abc123");
        let local = parse_invite_url("http://127.0.0.1:9857/p/v1/join/t").unwrap();
        assert_eq!(local.base, "http://127.0.0.1:9857");
        assert!(parse_invite_url("https://alex.example.com/p/v1/join").is_err());
        assert!(parse_invite_url("https://alex.example.com/f/contact").is_err());
        assert!(parse_invite_url("not a url").is_err());
    }

    #[test]
    fn auto_reply_counters_rise_and_a_manual_message_resets_them() {
        let root = home();
        let contact = contact();
        upsert_contact(&root, contact.clone()).unwrap();
        assert_eq!(note_auto_reply(&root, contact.id).unwrap(), 1);
        assert_eq!(note_auto_reply(&root, contact.id).unwrap(), 2);
        note_manual_message(&root, contact.id).unwrap();
        assert_eq!(load(&root).unwrap().contacts[0].auto_turns, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_contact_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = home();
        upsert_contact(&root, contact()).unwrap();
        let mode = std::fs::metadata(path(&root)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(root).unwrap();
    }
}
