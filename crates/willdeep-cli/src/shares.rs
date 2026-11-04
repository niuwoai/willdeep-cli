//! 会话分享：把一条会话挂到一个不透明 token 后面，外人只看到这条会话本身。
//!
//! 分享页由公网入口（`[intake]`）那个监听提供，和 Web 服务是两个监听：Web 没有
//! 应用层鉴权，能连上的人就是这个用户；分享页只有会话文本和一个输入框，没有
//! 配置、插件、工作区列表，也没有任何能改服务状态的端点。
//!
//! token 存在 `shares.json`（0600）。一台机器上一条会话最多一个 token，重新
//! 分享就换一个新的——旧链接当场失效，比「撤销再新建」少一次点击。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 分享出去的权限。`read` 只能看；`chat` 还能继续发消息。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShareMode {
    /// 只看会话。
    Read,
    /// 可以继续对话。访客发来的消息就是普通的一轮，按会话当前授权模式执行。
    Chat,
}

impl ShareMode {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Chat => "chat",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
    pub session_id: uuid::Uuid,
    pub mode: ShareMode,
    pub created_at: u64,
}

/// token → 分享记录。BTreeMap 让文件内容稳定，手工看也方便。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Shares(BTreeMap<String, Share>);

pub fn path(home: &Path) -> PathBuf {
    home.join("shares.json")
}

pub fn load(home: &Path) -> Shares {
    std::fs::read(path(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Shares>(&bytes).ok())
        .unwrap_or_default()
}

/// 新建一个分享，并撤掉这条会话原来的 token。
pub fn create(home: &Path, session_id: uuid::Uuid, mode: ShareMode) -> Result<(String, Share)> {
    let mut shares = load(home);
    shares.0.retain(|_, share| share.session_id != session_id);
    let token = new_token();
    let share = Share {
        session_id,
        mode,
        created_at: now_seconds(),
    };
    shares.0.insert(token.clone(), share.clone());
    save(home, &shares)?;
    Ok((token, share))
}

pub fn remove(home: &Path, session_id: uuid::Uuid) -> Result<bool> {
    let mut shares = load(home);
    let before = shares.0.len();
    shares.0.retain(|_, share| share.session_id != session_id);
    if shares.0.len() == before {
        return Ok(false);
    }
    save(home, &shares)?;
    Ok(true)
}

/// 会话当前对应的 token。一条会话最多一个。
pub fn token_for(home: &Path, session_id: uuid::Uuid) -> Option<(String, Share)> {
    load(home)
        .0
        .into_iter()
        .find(|(_, share)| share.session_id == session_id)
}

pub fn by_token(home: &Path, token: &str) -> Option<(String, Share)> {
    load(home)
        .0
        .remove(token)
        .map(|share| (token.to_owned(), share))
}

/// 公开访问路径。监听地址为 `0.0.0.0` 时让调用方换成本机域名——这里只管拼，
/// 不知道别人从哪个域名进来。
pub fn public_path(token: &str) -> String {
    format!("/share/{token}")
}

fn save(home: &Path, shares: &Shares) -> Result<()> {
    std::fs::create_dir_all(home)?;
    let target = path(home);
    let temporary = home.join(format!(".shares-{}.json", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(shares).context("encode shares")?;
    std::fs::write(&temporary, bytes).context("write shares")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, &target).context("replace shares")?;
    Ok(())
}

fn new_token() -> String {
    // 两个 v4 拼起来是 256 位随机量；写进 URL，不需要可读。
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// 分享页能发的消息长度上限。和表单字段同一量级：够说清一件事，不够塞一篇稿。
pub const MAX_SHARE_MESSAGE_CHARS: usize = 8 * 1024;

/// token 的形状检查。URL 里只可能出现这一种 token，别的形状直接 404。
pub fn plausible_token(token: &str) -> bool {
    (8..=128).contains(&token.len())
        && token
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '-')
}

pub fn validate_mode(mode: ShareMode) -> Result<()> {
    match mode {
        ShareMode::Read | ShareMode::Chat => Ok(()),
    }
}

/// 分享链接要求会话存在且工作区还在白名单里。这里给出统一的拒绝文案，
/// 免得 Web 与入口各写一套、口径不一。
pub fn ensure_session(
    home: &Path,
    session_id: uuid::Uuid,
) -> Result<willdeep_core::session::Session> {
    let session = willdeep_core::session::SessionStore::new(home)
        .load(session_id)
        .map_err(|_| anyhow::anyhow!("session was not found"))?;
    anyhow::ensure!(
        session.parent_session_id.is_none() && !session.delegation_read_only,
        "delegated sessions are private"
    );
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("willdeep-shares-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn creating_a_share_replaces_the_session_old_link() {
        let home = home();
        let session = uuid::Uuid::new_v4();
        let (first, _) = create(&home, session, ShareMode::Read).unwrap();
        let (second, share) = create(&home, session, ShareMode::Chat).unwrap();

        assert_ne!(first, second);
        // 一条会话只有一个 token：旧链接当场失效，不是留着一个还能用的。
        assert!(by_token(&home, &first).is_none());
        assert_eq!(
            by_token(&home, &second).map(|(_, share)| share.mode),
            Some(ShareMode::Chat)
        );
        assert_eq!(
            token_for(&home, session).map(|(token, _)| token),
            Some(second)
        );
        assert_eq!(share.session_id, session);
    }

    #[test]
    fn removing_a_share_takes_the_token_with_it() {
        let home = home();
        let session = uuid::Uuid::new_v4();
        let (token, _) = create(&home, session, ShareMode::Read).unwrap();

        assert!(remove(&home, session).unwrap());
        assert!(by_token(&home, &token).is_none());
        // 再删一次是幂等的「没有」，不是错误。
        assert!(!remove(&home, session).unwrap());
    }

    #[test]
    fn other_sessions_keep_their_share_when_one_is_removed() {
        let home = home();
        let keep = uuid::Uuid::new_v4();
        let drop = uuid::Uuid::new_v4();
        let (kept, _) = create(&home, keep, ShareMode::Read).unwrap();
        create(&home, drop, ShareMode::Chat).unwrap();

        remove(&home, drop).unwrap();
        assert!(by_token(&home, &kept).is_some());
    }

    #[test]
    fn only_url_shaped_tokens_are_plausible() {
        assert!(plausible_token("abcdefgh"));
        assert!(plausible_token(&"a".repeat(128)));
        assert!(!plausible_token("abc"));
        assert!(!plausible_token(&"a".repeat(129)));
        assert!(!plausible_token("../../etc/passwd"));
        assert!(!plausible_token("token with space"));
    }

    #[test]
    fn public_path_is_the_intake_route() {
        assert_eq!(public_path("abc12345"), "/share/abc12345");
    }
}
