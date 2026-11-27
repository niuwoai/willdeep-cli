//! 用户 MCP 定义的独立存放。
//!
//! 每个服务一个文件：`$WILLDEEP_HOME/.mcp/<name>.toml`，内容就是原来
//! `[mcp_servers.<name>]` 表里的字段，没有表头。主配置 `config.toml` 不再
//! 带 `[mcp_servers.*]`——改一个服务写坏文件，不能再让整份模型配置解析失败。
//!
//! 旧配置仍能启动：主配置里残留的 `[mcp_servers.*]` 在加载时迁进独立文件，
//! 再从主配置里删掉。单个独立文件解析或校验失败只跳过并告警，其余服务照常。

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use willdeep_core::McpServerConfig;

use super::willdeep_home;

pub(crate) const DIRECTORY_NAME: &str = ".mcp";
const LOCK_NAME: &str = ".lock";

/// 一次加载的结果。`skipped` 是读到了但没采用的文件，调用方负责告诉用户。
#[derive(Clone, Debug, Default)]
pub(crate) struct McpLoad {
    pub servers: BTreeMap<String, McpServerConfig>,
    pub skipped: Vec<String>,
}

/// `$WILLDEEP_HOME/.mcp`。`config_path` 给出时按其父目录算家目录，
/// 这样显式 `--config` 和测试夹具不会去碰用户的真目录。
pub(crate) fn directory_for(config_path: Option<&Path>) -> Result<PathBuf> {
    let home = match config_path {
        Some(path) => path
            .parent()
            .context("configuration path has no parent")?
            .to_path_buf(),
        None => willdeep_home()?,
    };
    Ok(home.join(DIRECTORY_NAME))
}

/// 服务名能做文件名的部分。与配置里其他实例名同一条线：字母、数字、
/// `_`、`-`，1 到 64。拒绝 `.` 与路径分隔符，免得 `../` 写出目录。
pub(crate) fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

pub(crate) fn file_for(directory: &Path, name: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        bail!(
            "MCP server name {name:?} must be 1 to 64 characters of ASCII letters, numbers, '_' or '-'"
        );
    }
    Ok(directory.join(format!("{name}.toml")))
}

/// 读目录里的全部服务。目录不存在就是没有服务，不是错误。
pub(crate) fn load_directory(directory: &Path) -> McpLoad {
    let mut loaded = McpLoad::default();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return loaded,
        Err(error) => {
            loaded
                .skipped
                .push(format!("{}: {error}", directory.display()));
            return loaded;
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("toml") {
            paths.push(path);
        }
    }
    paths.sort();
    for path in paths {
        match load_file(&path) {
            Ok((name, config)) => {
                loaded.servers.insert(name, config);
            }
            Err(error) => loaded.skipped.push(format!("{error:#}")),
        }
    }
    loaded
}

/// 明文 token 所在的文件必须只有属主可读。我们写出的文件是 0600；用户
/// 手写后放到共享目录里的，在读到 token 时就拒绝，而不是等发出去。
fn ensure_token_file_is_private(path: &Path, config: &McpServerConfig) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if config
            .token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
        {
            let mode = fs::metadata(path)?.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                bail!(
                    "{} holds a token but permissions are {mode:o}; run `chmod 600 {}` or move the token to bearer_token_env",
                    path.display(),
                    path.display()
                );
            }
        }
    }
    let _ = (path, config);
    Ok(())
}

/// 报错文本里凡是 `token = ...` 的赋值，值换成占位。行内可能夹着文件原文。
fn redact_token_lines(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("token") {
        let (head, after) = rest.split_at(at + "token".len());
        output.push_str(head);
        let trimmed = after.trim_start();
        if let Some(stripped) = trimmed.strip_prefix('=') {
            let value = stripped.trim_start();
            let end = if let Some(quoted) = value.strip_prefix('"') {
                quoted
                    .find('"')
                    .map(|offset| value.len() - quoted.len() + offset + 1)
            } else if let Some(quoted) = value.strip_prefix('\'') {
                quoted
                    .find('\'')
                    .map(|offset| value.len() - quoted.len() + offset + 1)
            } else {
                Some(value.find([',', '\n', '}']).unwrap_or(value.len()))
            };
            if let Some(end) = end {
                output.push_str(" = [REDACTED]");
                rest = &value[end..];
                continue;
            }
        }
        rest = after;
    }
    output.push_str(rest);
    output
}

fn load_file(path: &Path) -> Result<(String, McpServerConfig)> {
    let name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .context("MCP file name is not valid UTF-8")?;
    if !valid_name(name) {
        bail!(
            "{}: server name {name:?} must be 1 to 64 characters of ASCII letters, numbers, '_' or '-'",
            path.display()
        );
    }
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read MCP server file: {}", path.display()))?;
    let config: McpServerConfig = toml::from_str(&contents).map_err(|error| {
        // toml 的报错会引用出错那一行的原文。token 行写错时不能把值带出去。
        anyhow::anyhow!(
            "parse MCP server file {}: {} — a field here is not part of an MCP server (command, args, env, url, headers, token, bearer_token_env, oauth, startup_timeout_seconds, enabled)",
            path.display(),
            redact_token_lines(&error.to_string())
        )
    })?;
    config
        .validate(name)
        .with_context(|| format!("validate MCP server file: {}", path.display()))?;
    ensure_token_file_is_private(path, &config)?;
    Ok((name.to_owned(), config))
}

/// 把一个服务写成独立文件。调用方必须已经持有 [`lock`]。
pub(crate) fn write_server(
    directory: &Path,
    name: &str,
    config: &McpServerConfig,
) -> Result<PathBuf> {
    let path = file_for(directory, name)?;
    fs::create_dir_all(directory)
        .with_context(|| format!("create MCP directory: {}", directory.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).with_context(|| {
            format!(
                "restrict MCP directory permissions: {}",
                directory.display()
            )
        })?;
    }
    let body = render_server(config)?;
    write_private_atomic(&path, body.as_bytes())?;
    Ok(path)
}

/// 删掉一个服务的文件。文件本来就不在，不算失败。
pub(crate) fn remove_server(directory: &Path, name: &str) -> Result<bool> {
    let path = file_for(directory, name)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("remove MCP server file: {}", path.display()))
        }
    }
}

/// 目录锁。持有期间别人不能迁移或改写服务文件；进程退出后由系统释放。
pub(crate) fn lock(directory: &Path) -> Result<std::fs::File> {
    fs::create_dir_all(directory)
        .with_context(|| format!("create MCP directory: {}", directory.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).ok();
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // 锁文件的 inode 必须稳定：删掉它，另一个进程会锁上新文件，而旧的
    // 描述符还握着旧锁，两把锁就不再互斥。
    let file = options
        .open(directory.join(LOCK_NAME))
        .with_context(|| format!("open MCP directory lock: {}", directory.display()))?;
    file.lock()
        .with_context(|| format!("lock MCP directory: {}", directory.display()))?;
    Ok(file)
}

/// 把主配置里读懂的 `[mcp_servers.*]` 抽成独立文件。
///
/// 返回这次新写出的服务名。同名文件已存在就不覆盖：独立文件是之后改过的
/// 那一份，主配置里的残留是迁移前的旧副本，不在返回值里。调用方只有在
/// 主配置改写成功之后才能把这些段从主配置里删掉；改写失败按返回的名字回滚。
pub(crate) fn extract_legacy(
    directory: &Path,
    _contents: &str,
    servers: &BTreeMap<String, McpServerConfig>,
) -> Result<Vec<String>> {
    if servers.is_empty() {
        return Ok(Vec::new());
    }
    let _guard = lock(directory)?;
    let mut written = Vec::new();
    for (name, config) in servers {
        let path = file_for(directory, name)?;
        if path.exists() {
            eprintln!(
                "warning: MCP server {name} is both in config.toml and {}; keeping the standalone file",
                path.display()
            );
            continue;
        }
        write_server(directory, name, config)?;
        written.push(name.clone());
    }
    Ok(written)
}

fn render_server(config: &McpServerConfig) -> Result<String> {
    // 交给 toml 序列化，布尔、数字和字符串的写法只此一份，不再手拼。
    // 新文件的默认凭据是调用方放进 `token` 的明文。这里不把 `bearer_token_env` 展开：
    // 迁移时用户明确写的是环境变量名，就原样写回去，不把当时环境里的值落进文件。
    let mut table = toml::map::Map::new();
    if let Some(command) = &config.command {
        table.insert("command".to_owned(), toml::Value::String(command.clone()));
    }
    if !config.args.is_empty() {
        table.insert(
            "args".to_owned(),
            toml::Value::Array(
                config
                    .args
                    .iter()
                    .cloned()
                    .map(toml::Value::String)
                    .collect(),
            ),
        );
    }
    if !config.env.is_empty() {
        table.insert("env".to_owned(), string_table(&config.env));
    }
    if let Some(url) = &config.url {
        table.insert("url".to_owned(), toml::Value::String(url.clone()));
    }
    if !config.headers.is_empty() {
        table.insert("headers".to_owned(), string_table(&config.headers));
    }
    if let Some(token) = &config.token {
        table.insert("token".to_owned(), toml::Value::String(token.clone()));
    }
    if let Some(variable) = &config.bearer_token_env {
        table.insert(
            "bearer_token_env".to_owned(),
            toml::Value::String(variable.clone()),
        );
    }
    if let Some(oauth) = &config.oauth {
        let mut oauth_table = toml::map::Map::new();
        if let Some(client_id) = &oauth.client_id {
            oauth_table.insert(
                "client_id".to_owned(),
                toml::Value::String(client_id.clone()),
            );
        }
        if let Some(variable) = &oauth.client_secret_env {
            oauth_table.insert(
                "client_secret_env".to_owned(),
                toml::Value::String(variable.clone()),
            );
        }
        if !oauth.scopes.is_empty() {
            oauth_table.insert(
                "scopes".to_owned(),
                toml::Value::Array(
                    oauth
                        .scopes
                        .iter()
                        .cloned()
                        .map(toml::Value::String)
                        .collect(),
                ),
            );
        }
        table.insert("oauth".to_owned(), toml::Value::Table(oauth_table));
    }
    table.insert(
        "startup_timeout_seconds".to_owned(),
        toml::Value::Integer(i64::try_from(config.startup_timeout_seconds).unwrap_or(i64::MAX)),
    );
    table.insert("enabled".to_owned(), toml::Value::Boolean(config.enabled));
    Ok(toml::to_string(&toml::Value::Table(table))?)
}

fn string_table(values: &BTreeMap<String, String>) -> toml::Value {
    toml::Value::Table(
        values
            .iter()
            .map(|(key, value)| (key.clone(), toml::Value::String(value.clone())))
            .collect(),
    )
}

fn write_private_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("MCP path has no parent: {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("server.toml");
    let temporary = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write_result = (|| -> Result<()> {
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("create temporary MCP file: {}", temporary.display()))?;
        file.write_all(contents)
            .with_context(|| format!("write temporary MCP file: {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary MCP file: {}", temporary.display()))?;
        fs::rename(&temporary, path)
            .with_context(|| format!("replace MCP file atomically: {}", path.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("willdeep-mcp-files-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn round_trips_stdio_and_http_servers() {
        let root = scratch();
        let directory = root.join(DIRECTORY_NAME);
        let stdio = McpServerConfig {
            command: Some("npx".to_owned()),
            args: vec!["-y".to_owned(), "server".to_owned()],
            env: BTreeMap::from([("NODE_NO_WARNINGS".to_owned(), "1".to_owned())]),
            startup_timeout_seconds: 30,
            enabled: false,
            ..McpServerConfig::default()
        };
        let http = McpServerConfig {
            url: Some("https://mcp.example.com/mcp".to_owned()),
            bearer_token_env: Some("ISSUES_TOKEN".to_owned()),
            headers: BTreeMap::from([("X-Trace".to_owned(), "willdeep".to_owned())]),
            ..McpServerConfig::default()
        };
        let inline = McpServerConfig {
            url: Some("https://panel.example.com/mcp".to_owned()),
            token: Some("inline-secret-value".to_owned()),
            ..McpServerConfig::default()
        };
        write_server(&directory, "filesystem", &stdio).unwrap();
        write_server(&directory, "issues", &http).unwrap();
        write_server(&directory, "panel", &inline).unwrap();

        let loaded = load_directory(&directory);
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        assert_eq!(loaded.servers["filesystem"].command.as_deref(), Some("npx"));
        assert_eq!(loaded.servers["filesystem"].args, stdio.args);
        assert_eq!(
            loaded.servers["filesystem"]
                .env
                .get("NODE_NO_WARNINGS")
                .map(String::as_str),
            Some("1")
        );
        assert!(!loaded.servers["filesystem"].enabled);
        assert_eq!(
            loaded.servers["issues"].url.as_deref(),
            Some("https://mcp.example.com/mcp")
        );
        assert_eq!(
            loaded.servers["issues"].bearer_token_env.as_deref(),
            Some("ISSUES_TOKEN")
        );
        assert_eq!(
            loaded.servers["panel"].token.as_deref(),
            Some("inline-secret-value")
        );
        let written = fs::read_to_string(directory.join("panel.toml")).unwrap();
        assert!(
            written.contains("token = \"inline-secret-value\""),
            "{written}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_broken_token_line_does_not_echo_the_value() {
        let root = scratch();
        let directory = root.join(DIRECTORY_NAME);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("panel.toml"),
            "url = \"https://panel.example.com/mcp\"\ntoken = \"super-secret-value\"\nnot_a_field = true\n",
        )
        .unwrap();
        let loaded = load_directory(&directory);
        assert!(loaded.servers.is_empty());
        let skipped = loaded.skipped.join("\n");
        assert!(!skipped.contains("super-secret-value"), "{skipped}");
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_token_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch();
        let directory = root.join(DIRECTORY_NAME);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("panel.toml");
        fs::write(
            &path,
            "url = \"https://panel.example.com/mcp\"\ntoken = \"super-secret-value\"\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = load_directory(&directory);
        assert!(loaded.servers.is_empty());
        let skipped = loaded.skipped.join("\n");
        assert!(skipped.contains("644"), "{skipped}");
        assert!(!skipped.contains("super-secret-value"), "{skipped}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_file_does_not_drop_the_others() {
        let root = scratch();
        let directory = root.join(DIRECTORY_NAME);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("good.toml"),
            "command = \"npx\"\nstartup_timeout_seconds = 10\nenabled = true\n",
        )
        .unwrap();
        fs::write(directory.join("broken.toml"), "transport = \"sse\"\n").unwrap();
        fs::write(directory.join("not a server.toml"), "command = \"npx\"\n").unwrap();

        let loaded = load_directory(&directory);
        assert_eq!(loaded.servers.len(), 1);
        assert!(loaded.servers.contains_key("good"));
        assert_eq!(loaded.skipped.len(), 2, "{:?}", loaded.skipped);
        assert!(
            loaded
                .skipped
                .iter()
                .any(|line| line.contains("broken.toml")),
            "{:?}",
            loaded.skipped
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn names_cannot_escape_the_directory() {
        let directory = PathBuf::from("/tmp/willdeep-mcp");
        for name in ["../config", "a/b", "a.b", "", "has space", "名字"] {
            assert!(
                file_for(&directory, name).is_err(),
                "{name} should be rejected"
            );
        }
        assert_eq!(
            file_for(&directory, "dingtalk").unwrap(),
            directory.join("dingtalk.toml")
        );
    }

    #[test]
    fn missing_directory_is_empty_not_an_error() {
        let loaded = load_directory(Path::new("/no/such/willdeep-mcp-dir"));
        assert!(loaded.servers.is_empty());
        assert!(loaded.skipped.is_empty());
    }
}
