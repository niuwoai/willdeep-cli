//! 公网入口（站点、表单、webhook）的管理面。
//!
//! 配置本体仍写在 `config.toml` 的 `[intake]` 段里——入口服务按文件启动，页面上
//! 改完要立刻按新配置重开监听，所以这里保存完会叫 Runtime 里的入口服务重新 apply
//! 一次。独立跑 `willdeep web` 时拿不到那个句柄，只保存并如实提示要重启。
//!
//! 提交记录分两头：成功的提交落成会话，绑定关系在 `intake-sessions.json`；
//! 失败的那部分由入口服务写进 `intake-pending.json`。两边都只读展示。

use super::*;
use crate::config::{IntakeField, IntakeForm, IntakeSettings, IntakeSite, IntakeWebhook};

pub(super) fn routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/intake", get(read).put(write))
        .route("/api/intake/browse", get(browse))
}

/// 表单字段。请求与响应共用一份：页面上编辑的就是入口服务读的那套拼写。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldView {
    name: String,
    label: String,
    /// `text`、`textarea`、`email`、`choice`。
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    required: bool,
    /// 只对 `choice` 有意义。
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteView {
    slug: String,
    root: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormView {
    slug: String,
    title: String,
    workspace: String,
    /// `continue` 续在同一个会话，`new` 每条提交新开一个。
    session: String,
    prompt: String,
    #[serde(default)]
    fields: Vec<FieldView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebhookView {
    slug: String,
    workspace: String,
    session: String,
    prompt: String,
    /// 密钥从哪个环境变量读；密钥本身不进配置文件，也不经这个页面。
    secret_env: String,
}

#[derive(Serialize)]
struct IntakeView {
    enabled: bool,
    listen: String,
    /// 配置里开着，但监听起没起来要问 Runtime——端口被占、配置非法都会让它不在跑。
    running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    running_listen: Option<String>,
    /// 由 Runtime 托管时才能立刻生效；否则保存后要重启。
    runtime_managed: bool,
    /// 这个页面能不能改（只有本机回环可以）。
    writable: bool,
    sites: Vec<SiteView>,
    forms: Vec<FormView>,
    webhooks: Vec<WebhookView>,
    /// 失败投递：谁、什么时候、为什么。
    pending: Vec<PendingRecord>,
    /// 表单与 webhook 现在绑到哪个会话。
    bindings: Vec<BindingView>,
}

#[derive(Serialize, Deserialize)]
struct PendingRecord {
    at: u64,
    slug: String,
    kind: String,
    reason: String,
}

#[derive(Serialize)]
struct BindingView {
    slug: String,
    session_id: String,
}

#[derive(Serialize)]
struct BrowseView {
    path: String,
    directories: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntakeRequest {
    enabled: bool,
    listen: String,
    #[serde(default)]
    sites: Vec<SiteView>,
    #[serde(default)]
    forms: Vec<FormView>,
    #[serde(default)]
    webhooks: Vec<WebhookView>,
}

#[derive(Deserialize)]
struct BrowseQuery {
    #[serde(default)]
    path: Option<String>,
}

impl IntakeRequest {
    /// 页面上填的东西是给人看的，交给入口服务之前转成配置里那套类型。
    fn settings(&self) -> IntakeSettings {
        IntakeSettings {
            enabled: self.enabled,
            listen: self.listen.trim().to_owned(),
            sites: self
                .sites
                .iter()
                .map(|site| IntakeSite {
                    slug: site.slug.trim().to_owned(),
                    root: PathBuf::from(site.root.trim()),
                })
                .collect(),
            forms: self
                .forms
                .iter()
                .map(|form| IntakeForm {
                    slug: form.slug.trim().to_owned(),
                    title: form.title.trim().to_owned(),
                    workspace: PathBuf::from(form.workspace.trim()),
                    session: form.session.trim().to_owned(),
                    prompt: form.prompt.trim().to_owned(),
                    fields: form
                        .fields
                        .iter()
                        .map(|field| IntakeField {
                            name: field.name.trim().to_owned(),
                            label: field.label.trim().to_owned(),
                            kind: field.kind.trim().to_owned(),
                            required: field.required,
                            options: field
                                .options
                                .iter()
                                .map(|option| option.trim().to_owned())
                                .filter(|option| !option.is_empty())
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
            webhooks: self
                .webhooks
                .iter()
                .map(|hook| IntakeWebhook {
                    slug: hook.slug.trim().to_owned(),
                    workspace: PathBuf::from(hook.workspace.trim()),
                    session: hook.session.trim().to_owned(),
                    prompt: hook.prompt.trim().to_owned(),
                    secret_env: hook.secret_env.trim().to_owned(),
                })
                .collect(),
        }
    }
}

fn site_views(settings: &IntakeSettings) -> Vec<SiteView> {
    settings
        .sites
        .iter()
        .map(|site| SiteView {
            slug: site.slug.clone(),
            root: site.root.display().to_string(),
        })
        .collect()
}

fn form_views(settings: &IntakeSettings) -> Vec<FormView> {
    settings
        .forms
        .iter()
        .map(|form| FormView {
            slug: form.slug.clone(),
            title: form.title.clone(),
            workspace: form.workspace.display().to_string(),
            session: form.session.clone(),
            prompt: form.prompt.clone(),
            fields: form
                .fields
                .iter()
                .map(|field| FieldView {
                    name: field.name.clone(),
                    label: field.label.clone(),
                    kind: field.kind.clone(),
                    required: field.required,
                    options: field.options.clone(),
                })
                .collect(),
        })
        .collect()
}

fn webhook_views(settings: &IntakeSettings) -> Vec<WebhookView> {
    settings
        .webhooks
        .iter()
        .map(|hook| WebhookView {
            slug: hook.slug.clone(),
            workspace: hook.workspace.display().to_string(),
            session: hook.session.clone(),
            prompt: hook.prompt.clone(),
            secret_env: hook.secret_env.clone(),
        })
        .collect()
}

/// 站点发布只需要一个目录，所以这里只列目录，不列文件。
async fn browse(
    State(state): State<Arc<WebState>>,
    Query(query): Query<BrowseQuery>,
) -> Result<Json<BrowseView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "browsing directories is available only from a loopback Web server",
        ));
    }
    let path = match query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => PathBuf::from(value),
        None => state.home.clone(),
    };
    let path = std::path::absolute(&path)
        .map_err(|error| WebError::bad_request(format!("invalid path: {error}")))?;
    let entries = std::fs::read_dir(&path).map_err(|error| {
        WebError::bad_request(format!("cannot read {}: {error}", path.display()))
    })?;
    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // 隐藏目录没有浏览价值，一屏塞满 .git / .cargo 反而更难找。
        if name.starts_with('.') {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            directories.push(name);
        }
    }
    directories.sort();
    directories.truncate(200);
    Ok(Json(BrowseView {
        path: path.display().to_string(),
        directories,
    }))
}

async fn read(State(state): State<Arc<WebState>>) -> Result<Json<IntakeView>, WebError> {
    let settings = load_settings(&state.config_path)?;
    let running_listen = match state.intake.as_ref() {
        Some(intake) => intake.status().await.map(|address| address.to_string()),
        None => None,
    };
    Ok(Json(IntakeView {
        enabled: settings.enabled,
        listen: settings.listen.clone(),
        running: running_listen.is_some(),
        running_listen,
        runtime_managed: state.intake.is_some(),
        writable: state.settings_writable,
        sites: site_views(&settings),
        forms: form_views(&settings),
        webhooks: webhook_views(&settings),
        pending: load_pending(&state.home),
        bindings: load_bindings(&state.home),
    }))
}

async fn write(
    State(state): State<Arc<WebState>>,
    Json(request): Json<IntakeRequest>,
) -> Result<Json<IntakeView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "public intake settings are available only from a loopback Web server",
        ));
    }
    let settings = request.settings();
    crate::config::validate_intake(&settings)
        .map_err(|error| WebError::bad_request(error.to_string()))?;
    // 站点目录必须是真实目录：配一个打不开的路径，入口服务会一直 404 而没有解释。
    for site in &settings.sites {
        if !site.root.is_dir() {
            return Err(WebError::bad_request(format!(
                "site {} root is not a directory: {}",
                site.slug,
                site.root.display()
            )));
        }
    }
    let guard = state.settings_write_lock.lock().await;
    save_settings(&state.config_path, &state.home, &settings).map_err(WebError::from_anyhow)?;
    if let Some(intake) = state.intake.as_ref() {
        // 配置已经生效了；重开监听失败时入口服务会自己打日志，页面状态照实反映。
        intake.apply(&state.config_path).await;
    }
    drop(guard);
    read(State(state)).await
}

/// 读 `[intake]`。配置文件里没有这一段时按默认值（关闭、回环地址）。
fn load_settings(path: &std::path::Path) -> Result<IntakeSettings, WebError> {
    match crate::config::LoadedConfig::load(Some(path)) {
        Ok(loaded) => Ok(loaded.file.intake),
        Err(error) => Err(WebError::from_anyhow(anyhow::anyhow!(
            "cannot read configuration: {error:#}"
        ))),
    }
}

fn load_pending(home: &std::path::Path) -> Vec<PendingRecord> {
    let Ok(bytes) = std::fs::read(home.join("intake-pending.json")) else {
        return Vec::new();
    };
    serde_json::from_slice::<Vec<PendingRecord>>(&bytes).unwrap_or_default()
}

fn load_bindings(home: &std::path::Path) -> Vec<BindingView> {
    #[derive(Deserialize)]
    struct Binding {
        session_id: uuid::Uuid,
    }
    let Ok(bytes) = std::fs::read(home.join("intake-sessions.json")) else {
        return Vec::new();
    };
    let Ok(stored) = serde_json::from_slice::<HashMap<String, Binding>>(&bytes) else {
        return Vec::new();
    };
    let mut bindings: Vec<BindingView> = stored
        .into_iter()
        .map(|(key, binding)| BindingView {
            slug: key,
            session_id: binding.session_id.to_string(),
        })
        .collect();
    bindings.sort_by(|a, b| a.slug.cmp(&b.slug));
    bindings
}

/// 把 `[intake]` 段整体换掉，其余内容和注释原样保留。
///
/// 只做行级手术：凡是 `[intake]`、`[intake.*]`、`[[intake.*]]` 开头的段落连同
/// 段内内容一起摘掉，再把新序列化的块追加到文件末尾。段内的注释会一起消失
/// ——页面上会提示这一点。
fn splice_intake(original: &str, block: &str) -> String {
    let mut kept = String::with_capacity(original.len() + block.len());
    let mut dropping = false;
    for line in original.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            let header = trimmed
                .trim_start_matches("[[")
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .split('.')
                .map(str::trim)
                .collect::<Vec<_>>();
            dropping = header.first() == Some(&"intake");
        }
        if !dropping {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    let kept = kept.trim_end();
    if kept.is_empty() {
        block.to_owned()
    } else {
        format!("{kept}\n\n{block}")
    }
}

fn settings_block(settings: &IntakeSettings) -> anyhow::Result<String> {
    let sites: Vec<toml::Value> = settings
        .sites
        .iter()
        .map(|site| {
            let mut table = toml::Table::new();
            table.insert("slug".to_owned(), toml::Value::String(site.slug.clone()));
            table.insert(
                "root".to_owned(),
                toml::Value::String(site.root.display().to_string()),
            );
            toml::Value::Table(table)
        })
        .collect();
    let forms: Vec<toml::Value> = settings
        .forms
        .iter()
        .map(|form| {
            let fields: Vec<toml::Value> = form
                .fields
                .iter()
                .map(|field| {
                    let mut table = toml::Table::new();
                    table.insert("name".to_owned(), toml::Value::String(field.name.clone()));
                    table.insert("label".to_owned(), toml::Value::String(field.label.clone()));
                    table.insert("type".to_owned(), toml::Value::String(field.kind.clone()));
                    table.insert("required".to_owned(), toml::Value::Boolean(field.required));
                    if !field.options.is_empty() {
                        table.insert(
                            "options".to_owned(),
                            toml::Value::Array(
                                field
                                    .options
                                    .iter()
                                    .map(|option| toml::Value::String(option.clone()))
                                    .collect(),
                            ),
                        );
                    }
                    toml::Value::Table(table)
                })
                .collect();
            let mut table = toml::Table::new();
            table.insert("slug".to_owned(), toml::Value::String(form.slug.clone()));
            table.insert("title".to_owned(), toml::Value::String(form.title.clone()));
            table.insert(
                "workspace".to_owned(),
                toml::Value::String(form.workspace.display().to_string()),
            );
            table.insert(
                "session".to_owned(),
                toml::Value::String(form.session.clone()),
            );
            table.insert(
                "prompt".to_owned(),
                toml::Value::String(form.prompt.clone()),
            );
            table.insert("fields".to_owned(), toml::Value::Array(fields));
            toml::Value::Table(table)
        })
        .collect();
    let webhooks: Vec<toml::Value> = settings
        .webhooks
        .iter()
        .map(|hook| {
            let mut table = toml::Table::new();
            table.insert("slug".to_owned(), toml::Value::String(hook.slug.clone()));
            table.insert(
                "workspace".to_owned(),
                toml::Value::String(hook.workspace.display().to_string()),
            );
            table.insert(
                "session".to_owned(),
                toml::Value::String(hook.session.clone()),
            );
            table.insert(
                "prompt".to_owned(),
                toml::Value::String(hook.prompt.clone()),
            );
            table.insert(
                "secret_env".to_owned(),
                toml::Value::String(hook.secret_env.clone()),
            );
            toml::Value::Table(table)
        })
        .collect();
    let mut intake = toml::Table::new();
    intake.insert("enabled".to_owned(), toml::Value::Boolean(settings.enabled));
    intake.insert(
        "listen".to_owned(),
        toml::Value::String(settings.listen.clone()),
    );
    if !sites.is_empty() {
        intake.insert("sites".to_owned(), toml::Value::Array(sites));
    }
    if !forms.is_empty() {
        intake.insert("forms".to_owned(), toml::Value::Array(forms));
    }
    if !webhooks.is_empty() {
        intake.insert("webhooks".to_owned(), toml::Value::Array(webhooks));
    }
    let mut root = toml::Table::new();
    root.insert("intake".to_owned(), toml::Value::Table(intake));
    Ok(toml::to_string_pretty(&toml::Value::Table(root))?.trim_end().to_owned())
}

fn save_settings(
    config_path: &std::path::Path,
    home: &std::path::Path,
    settings: &IntakeSettings,
) -> anyhow::Result<()> {
    let original = std::fs::read_to_string(config_path).unwrap_or_default();
    let next = splice_intake(&original, &settings_block(settings)?);
    // 整个文件重新解析一遍：段外的内容一个字都没动，出错只可能是我们拼错了。
    toml::from_str::<crate::config::ConfigFile>(&next)
        .map_err(|error| anyhow::anyhow!("the edited configuration would not parse: {error}"))?;
    backup_config(config_path, home)?;
    write_atomic(config_path, &next)
}

/// 先写临时文件再 rename，中途失败不会留下半个配置文件。
fn write_atomic(path: &std::path::Path, text: &str) -> anyhow::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("configuration path has no parent"))?;
    let temporary = directory.join(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn backup_config(config_path: &std::path::Path, home: &std::path::Path) -> anyhow::Result<()> {
    if !config_path.is_file() {
        return Ok(());
    }
    let directory = home.join("plugin-data/willdeep-config/backups");
    std::fs::create_dir_all(&directory)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let name = config_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_owned());
    let target = directory.join(format!("{name}-{stamp}.bak"));
    if !target.exists() {
        std::fs::copy(config_path, &target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> IntakeSettings {
        IntakeSettings {
            enabled: true,
            listen: "127.0.0.1:9857".to_owned(),
            sites: vec![IntakeSite {
                slug: "blog".to_owned(),
                root: PathBuf::from("/srv/blog"),
            }],
            forms: vec![IntakeForm {
                slug: "apply".to_owned(),
                title: "申请".to_owned(),
                workspace: PathBuf::from("/srv/work"),
                session: "continue".to_owned(),
                prompt: "收到一份新申请".to_owned(),
                fields: vec![IntakeField {
                    name: "email".to_owned(),
                    label: "邮箱".to_owned(),
                    kind: "email".to_owned(),
                    required: true,
                    options: Vec::new(),
                }],
            }],
            webhooks: Vec::new(),
        }
    }

    #[test]
    fn splice_replaces_only_the_intake_sections() {
        let original = "# 顶部注释\n[agent]\napproval = \"smart\"\n\n[intake]\nenabled = false\n\n[[intake.sites]]\nslug = \"old\"\nroot = \"/old\"\n\n[ui]\nmode = \"full\"\n";
        let next = splice_intake(original, &settings_block(&settings()).unwrap());
        assert!(next.contains("# 顶部注释"));
        assert!(next.contains("[agent]"));
        assert!(next.contains("[ui]"));
        assert!(!next.contains("slug = \"old\""));
        assert!(next.contains("[[intake.sites]]") || next.contains("[intake.sites]"));
        let parsed: toml::Value = toml::from_str(&next).expect("edited configuration parses");
        assert_eq!(
            parsed.get("intake").and_then(|intake| intake.get("enabled")),
            Some(&toml::Value::Boolean(true))
        );
        assert_eq!(
            parsed
                .get("agent")
                .and_then(|agent| agent.get("approval"))
                .and_then(toml::Value::as_str),
            Some("smart")
        );
    }

    #[test]
    fn empty_file_gets_just_the_intake_block() {
        let block = "[intake]\nenabled = false\nlisten = \"127.0.0.1:9857\"";
        assert_eq!(splice_intake("", block), block);
    }
}
