//! 实例身份：这台 willdeep 叫谁。
//!
//! 名字是给人看的标签，UUID 才是身份。改名不换 id——以后配对、审计、日志都认
//! id，名字随时可改。两者落在 `$WILLDEEP_HOME/identity.json`（0600），第一次用到
//! 时自动生成；文件坏了直接报错，不重新生成一个把旧身份顶掉。
//!
//! 这和遥测的 `telemetry-install-id` 不是一回事：那个是安装统计用的，关掉遥测
//! 就不会读；拿它当实例身份会让用户以为「关遥测等于换了个实例」。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const IDENTITY_SCHEMA: &str = "willdeep.identity.v1";
/// 名字的长度上限。侧栏、状态行和以后的消息来源行都要放得下。
pub const MAX_NAME_CHARS: usize = 24;

/// 首次运行的名字池。纯 ASCII、小写、没有相近拼写，念出来就能打对。
pub const NAME_POOL: &[&str] = &[
    "ada", "alex", "arlo", "bella", "bruno", "cato", "cleo", "dara", "dina", "eli", "emil", "esme",
    "felix", "fiona", "freya", "gabe", "gus", "hana", "hazel", "hugo", "ines", "iris", "ivo",
    "jade", "jack", "jonas", "june", "kai", "kira", "kiro", "lena", "leo", "lin", "luna", "marco",
    "max", "milo", "mira", "nessa", "nina", "noor", "nova", "orla", "oscar", "otis", "owen", "pax",
    "priya", "quinn", "rey", "rosa", "ruby", "sasha", "smith", "taro", "theo", "uma", "vera",
    "wade", "wren", "xena", "yori", "zara", "zed",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub schema: String,
    /// 终身不变。名字换了、机器换了地址，认的还是它。
    pub id: uuid::Uuid,
    pub name: String,
    pub created_at: u64,
    /// 最后一次改名的时刻；从没改过就没有这一项。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_at: Option<u64>,
}

impl Identity {
    /// 展示用的名字。同一群人里没有重名时只显示名字；重名时补 UUID 末 4 位，
    /// 让「alex 是哪一个」有唯一答案。冲突判断由调用方给——只有群成员表知道
    /// 有没有另一个 alex。
    pub fn display_name(&self, ambiguous: bool) -> String {
        if ambiguous {
            format!("{}:{}", self.name, short_id(self.id))
        } else {
            self.name.clone()
        }
    }
}

/// UUID 末 4 位，小写十六进制。4 位仍撞车时调用方退到 8 位。
pub fn short_id(id: uuid::Uuid) -> String {
    let hex = id.simple().to_string();
    hex[hex.len() - 4..].to_owned()
}

pub fn path(home: &Path) -> PathBuf {
    home.join("identity.json")
}

/// 读身份；没有就当场生成一个并落盘。
pub fn load(home: &Path) -> Result<Identity> {
    match std::fs::read(path(home)) {
        Ok(bytes) => {
            let identity: Identity = serde_json::from_slice(&bytes)
                .context("identity.json is not a valid identity file")?;
            if identity.schema != IDENTITY_SCHEMA {
                bail!(
                    "identity.json uses schema {}, expected {IDENTITY_SCHEMA}",
                    identity.schema
                );
            }
            validate_name(&identity.name)
                .context("identity.json holds an invalid name")?;
            Ok(identity)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let identity = generate();
            save(home, &identity)?;
            Ok(identity)
        }
        Err(error) => Err(error).context("cannot read identity.json"),
    }
}

/// 改名，id 不动。返回写回之后的那份身份。
pub fn rename(home: &Path, name: &str) -> Result<Identity> {
    let name = validate_name(name)?;
    let mut identity = load(home)?;
    if identity.name == name {
        return Ok(identity);
    }
    identity.name = name;
    identity.renamed_at = Some(now_seconds());
    save(home, &identity)?;
    Ok(identity)
}

/// 名字的规则：1–24 个字符，ASCII 字母、数字、`-`、`_`。
///
/// 不放中文：名字会出现在消息来源行、日志和以后的 URL 参数里，纯 ASCII 省掉
/// 转义和宽度对齐两件事。冒号也不允许——`alex:2u2d` 里它是分隔符。
pub fn validate_name(value: &str) -> Result<String> {
    let name = value.trim();
    if name.is_empty() {
        bail!("the instance name cannot be empty");
    }
    if name.chars().count() > MAX_NAME_CHARS {
        bail!("the instance name must be at most {MAX_NAME_CHARS} characters");
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("the instance name may only use ASCII letters, digits, '-' and '_'");
    }
    if !name
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        bail!("the instance name must start with a letter or digit");
    }
    Ok(name.to_owned())
}

/// 首个身份：随机 UUID + 名字池里的一个名字。
fn generate() -> Identity {
    let id = uuid::Uuid::new_v4();
    let bytes = id.as_bytes();
    let index = usize::from(u16::from_le_bytes([bytes[0], bytes[1]])) % NAME_POOL.len();
    Identity {
        schema: IDENTITY_SCHEMA.to_owned(),
        id,
        name: NAME_POOL[index].to_owned(),
        created_at: now_seconds(),
        renamed_at: None,
    }
}

fn save(home: &Path, identity: &Identity) -> Result<()> {
    std::fs::create_dir_all(home).context("create the willdeep home directory")?;
    let target = path(home);
    let temporary = home.join(format!(".identity-{}.json", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(identity).context("encode identity")?;
    std::fs::write(&temporary, bytes).context("write identity")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .context("restrict identity file permissions")?;
    }
    std::fs::rename(&temporary, &target).context("replace identity")?;
    Ok(())
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        let root = std::env::temp_dir().join(format!("willdeep-identity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_missing_identity_is_generated_once_and_then_reused() {
        let root = home();
        let first = load(&root).unwrap();
        assert!(first.id != uuid::Uuid::nil());
        assert!(NAME_POOL.contains(&first.name.as_str()));
        assert!(first.renamed_at.is_none());

        let second = load(&root).unwrap();
        assert_eq!(first, second, "the second read must reuse the stored identity");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renaming_keeps_the_id_and_records_the_change() {
        let root = home();
        let first = load(&root).unwrap();
        let renamed = rename(&root, "  jack  ").unwrap();
        assert_eq!(renamed.name, "jack");
        assert_eq!(renamed.id, first.id);
        assert!(renamed.renamed_at.is_some());
        assert_eq!(load(&root).unwrap().name, "jack");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn names_are_ascii_without_the_display_separator() {
        assert_eq!(validate_name("alex").unwrap(), "alex");
        assert_eq!(validate_name("deep-2").unwrap(), "deep-2");
        assert!(validate_name("").is_err());
        assert!(validate_name("alex:2u2d").is_err());
        assert!(validate_name("张三").is_err());
        assert!(validate_name("-alex").is_err());
        assert!(validate_name(&"a".repeat(MAX_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn a_corrupt_identity_is_never_silently_replaced() {
        let root = home();
        std::fs::write(path(&root), b"{ not json").unwrap();
        assert!(load(&root).is_err());
        assert_eq!(std::fs::read(path(&root)).unwrap(), b"{ not json");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_duplicate_name_is_disambiguated_by_the_id_suffix() {
        let identity = Identity {
            schema: IDENTITY_SCHEMA.to_owned(),
            id: uuid::Uuid::parse_str("6f0a1111-2222-3333-4444-55550abc1234").unwrap(),
            name: "alex".to_owned(),
            created_at: 0,
            renamed_at: None,
        };
        assert_eq!(identity.display_name(false), "alex");
        assert_eq!(identity.display_name(true), format!("alex:{}", short_id(identity.id)));
    }

    #[cfg(unix)]
    #[test]
    fn the_identity_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = home();
        load(&root).unwrap();
        let mode = std::fs::metadata(path(&root)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(root).unwrap();
    }
}
