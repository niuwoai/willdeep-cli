//! Web MCP 端点的访问 token。
//!
//! 明文只在创建时返回一次；落盘的是 SHA-256。文件权限 0600，
//! 跟插件网关的发现文件一个待遇：谁能读到家目录，谁就等于持有 token。
//! 校验用逐字节异或累加，不在第一个不同处提前返回。

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FILE_NAME: &str = "mcp-tokens.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct McpTokenRecord {
    pub id: uuid::Uuid,
    /// 用户起的名字，只用来在列表里辨认。
    pub label: String,
    /// token 明文的 SHA-256，小写十六进制。
    pub hash: String,
    pub created_at: u64,
    /// 撤销时间。撤销不是删除：留着记得曾经发过，但不再能用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    tokens: Vec<McpTokenRecord>,
}

fn path_for(home: &Path) -> std::path::PathBuf {
    home.join(FILE_NAME)
}

fn read(home: &Path) -> File {
    std::fs::read(path_for(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write(home: &Path, file: &File) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    let body = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    willdeep_core::plugin::gateway::write_private(&path_for(home), &body)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 生成一个 token 并记下它的哈希。返回的明文是唯一一次能拿到它的机会。
pub(crate) fn issue(home: &Path, label: &str) -> std::io::Result<(McpTokenRecord, String)> {
    let secret = format!(
        "wdm_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let record = McpTokenRecord {
        id: uuid::Uuid::new_v4(),
        label: label.trim().chars().take(64).collect(),
        hash: hash(&secret),
        created_at: now(),
        revoked_at: None,
    };
    let mut file = read(home);
    file.tokens.push(record.clone());
    write(home, &file)?;
    Ok((record, secret))
}

pub(crate) fn list(home: &Path) -> Vec<McpTokenRecord> {
    read(home).tokens
}

/// 撤销一个 token。找不到返回 `false`，已经撤销过也算成功。
pub(crate) fn revoke(home: &Path, id: uuid::Uuid) -> std::io::Result<bool> {
    let mut file = read(home);
    let Some(record) = file.tokens.iter_mut().find(|record| record.id == id) else {
        return Ok(false);
    };
    if record.revoked_at.is_none() {
        record.revoked_at = Some(now());
        write(home, &file)?;
    }
    Ok(true)
}

/// token 是否有效：哈希对得上，且没有被撤销。
pub(crate) fn authenticate(home: &Path, presented: &str) -> bool {
    let hashed = hash(presented);
    read(home).tokens.iter().any(|record| {
        record.revoked_at.is_none() && constant_time_eq(record.hash.as_bytes(), hashed.as_bytes())
    })
}

fn hash(secret: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    Sha256::digest(secret.as_bytes())
        .iter()
        .flat_map(|byte| [HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]])
        .map(char::from)
        .collect()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "willdeep-mcp-tokens-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_fresh_token_authenticates_and_a_revoked_one_does_not() {
        let home = home("auth");
        let (record, secret) = issue(&home, "claude code").unwrap();
        assert!(secret.starts_with("wdm_"));
        assert!(authenticate(&home, &secret));
        assert!(!authenticate(&home, "wdm_nope"));

        // 落盘的是哈希，明文不在文件里。
        let stored = std::fs::read_to_string(path_for(&home)).unwrap();
        assert!(!stored.contains(&secret));

        assert!(revoke(&home, record.id).unwrap());
        assert!(!authenticate(&home, &secret));
        assert_eq!(list(&home).len(), 1, "撤销留着记录，不删掉");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn revoking_an_unknown_id_reports_it() {
        let home = home("missing");
        assert!(!revoke(&home, uuid::Uuid::new_v4()).unwrap());
        let _ = std::fs::remove_dir_all(&home);
    }
}
