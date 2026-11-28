//! 插件运行状态：启用与权限审批。
//!
//! 这份状态**不与 Xedit 共享**，只有包内容共享。理由：Web 宿主的沙箱边界与
//! macOS 原生宿主不同（这边是 opaque-origin iframe + CSP，那边是非持久化
//! WKWebView + 自定义协议），把审批当共享货币传过去，等于替另一个宿主替用户
//! 点了头。同一个包在两端各批一次，是成本，也是正确性。
//!
//! 文件权限固定 0600。看见 group/other 位就拒绝整个存储而不是"修好继续"——
//! 一个别人可写的审批记录，能让任意插件在下次启动时变成已批准。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::manifest::PluginPermission;
use super::package::{PackageError, PluginPackage};

const STATE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("cannot read plugin registry at {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("cannot write plugin registry at {path}: {reason}")]
    Write { path: String, reason: String },
    #[error(
        "plugin registry {path} is readable by group or others (mode {mode:o}); \
         fix it with chmod 600 before continuing"
    )]
    Permissions { path: String, mode: u32 },
    #[error("plugin registry at {path} is not valid JSON: {reason}")]
    Corrupt { path: String, reason: String },
    #[error("plugin registry at {path} is version {found}, this build writes {STATE_VERSION}")]
    UnknownVersion { path: String, found: u32 },
    #[error(transparent)]
    Package(#[from] super::package::PackageError),
}

/// 用户在插件中心查看的版本、来源及经过校验的包内容。
#[derive(Debug, Deserialize)]
pub struct PluginApprovalReview {
    pub version: String,
    pub source: String,
    pub digest: String,
}

/// 一次权限审批的记录。绑定四样东西：版本、内容 digest、来源与权限集合。
/// 任何一样变了都要重新确认——"我批准的是那个东西"必须可核对。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginApproval {
    pub version: String,
    pub digest: String,
    pub source: String,
    pub permissions: Vec<String>,
    pub approved_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginState {
    #[serde(default)]
    pub enabled: bool,
    /// A new enable cycle must not make previously issued asset grants valid again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<PluginApproval>,
    /// 固定入口的顺序；未固定的插件没有这个值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_order: Option<u32>,
    /// 是否在左侧栏给出入口。`None` 表示用户没表过态，按插件清单的默认值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_in_rail: Option<bool>,
    /// Values live in the private registry and are used for MCP interpolation.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
    /// Classify stored values independently of the currently discovered manifest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    setting_secrecy: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RegistryFile {
    version: u32,
    #[serde(default)]
    plugins: BTreeMap<String, PluginState>,
}

impl Default for RegistryFile {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            plugins: BTreeMap::new(),
        }
    }
}

/// 为什么一个插件不能启用。用于界面上直接说人话，而不是一个哑掉的开关。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalGap {
    /// 从来没批准过。
    NeverApproved,
    /// 批准过别的版本。
    VersionChanged { approved: String },
    /// 同版本但内容变了——比换版本更需要警惕。
    DigestChanged,
    /// 来源变了：同一个 ID 同一个版本，从别处来的不是同一个东西。
    SourceChanged { approved: String },
    /// 新增了没批准过的权限。
    NewPermissions(Vec<String>),
}

impl ApprovalGap {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NeverApproved => "never-approved",
            Self::VersionChanged { .. } => "version-changed",
            Self::DigestChanged => "digest-changed",
            Self::SourceChanged { .. } => "source-changed",
            Self::NewPermissions(_) => "new-permissions",
        }
    }
}

pub struct PluginRegistry {
    path: PathBuf,
    file: RegistryFile,
}

impl PluginRegistry {
    /// 默认位置：`~/.willdeep/plugin-registry.web.json`。名字里的 `web` 是提醒——
    /// 这是 rs 宿主自己的记录，不是 Xedit 那份。
    pub fn default_path(home: &Path) -> PathBuf {
        home.join("plugin-registry.web.json")
    }

    pub fn load(path: &Path) -> Result<Self, RegistryError> {
        if !path.exists() {
            return Ok(Self {
                path: path.to_path_buf(),
                file: RegistryFile::default(),
            });
        }
        check_permissions(path)?;
        let source = fs::read_to_string(path).map_err(|error| RegistryError::Read {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        let file: RegistryFile =
            serde_json::from_str(&source).map_err(|error| RegistryError::Corrupt {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?;
        // 未来版本写的文件这里读不懂。拒绝比"按 1 解释"安全：后者可能把一条
        // 新语义的审批当成旧语义放行。
        if file.version != STATE_VERSION {
            return Err(RegistryError::UnknownVersion {
                path: path.display().to_string(),
                found: file.version,
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    pub fn state(&self, plugin_id: &str) -> Option<&PluginState> {
        self.file.plugins.get(plugin_id)
    }

    pub fn is_enabled(&self, plugin_id: &str) -> bool {
        self.file
            .plugins
            .get(plugin_id)
            .is_some_and(|state| state.enabled)
    }

    /// 当前包与已有审批之间的差距。`None` 表示可以直接启用。
    ///
    /// 检查顺序是精心排的，也顺带避开了最贵的那一步：没批准过、版本变了、
    /// 权限多了，这三条都不必读包内容就能判定，只有走到 digest 比对时才需要
    /// 真的把包哈希一遍。从没批准过的插件（比如一个 26 MB 的 Codex 兼容包）
    /// 因此一个字节都不会被读。
    pub fn approval_gap(
        &self,
        package: &PluginPackage,
    ) -> Result<Option<ApprovalGap>, RegistryError> {
        self.approval_gap_with_contents(package, |_, _| {})
    }

    /// Like `approval_gap`, with access to the current bytes used for the digest.
    /// The caller may use observed bytes only if this returns `Ok(None)`.
    pub fn approval_gap_with_contents(
        &self,
        package: &PluginPackage,
        observe: impl FnMut(&str, &[u8]),
    ) -> Result<Option<ApprovalGap>, RegistryError> {
        self.approval_gap_with_digest(package, |package| package.digest_with_contents(observe))
    }

    /// 仅用于展示或能力提示的审批预览。可能复用旧摘要，不能据此放行操作；
    /// 启用和实际使用前仍须调用 `approval_gap` 读取当前内容。
    pub fn approval_gap_preview(
        &self,
        package: &PluginPackage,
    ) -> Result<Option<ApprovalGap>, RegistryError> {
        self.approval_gap_with_digest(package, PluginPackage::cached_digest)
    }

    pub fn reviewed_approval_gap(
        &self,
        package: &PluginPackage,
        digest: &str,
    ) -> Result<Option<ApprovalGap>, RegistryError> {
        self.approval_gap_with_digest(package, |_| Ok(digest.to_owned()))
    }

    fn approval_gap_with_digest(
        &self,
        package: &PluginPackage,
        digest: impl FnOnce(&PluginPackage) -> Result<String, PackageError>,
    ) -> Result<Option<ApprovalGap>, RegistryError> {
        let requested = permission_names(package);
        let Some(approval) = self
            .file
            .plugins
            .get(&package.id)
            .and_then(|state| state.approval.as_ref())
        else {
            return Ok(Some(ApprovalGap::NeverApproved));
        };
        if approval.version != package.version {
            return Ok(Some(ApprovalGap::VersionChanged {
                approved: approval.version.clone(),
            }));
        }
        // 权限差异排在 digest 之前是有意的：改权限必然改 digest，先报
        // DigestChanged 只会告诉用户"变了"，而他真正需要知道的是"它现在还想要
        // 网络访问"。理由越具体，那一次点头才越算数。
        let added: Vec<String> = requested
            .iter()
            .filter(|item| !approval.permissions.contains(item))
            .cloned()
            .collect();
        if !added.is_empty() {
            return Ok(Some(ApprovalGap::NewPermissions(added)));
        }
        if approval.source != package.source.as_str() {
            return Ok(Some(ApprovalGap::SourceChanged {
                approved: approval.source.clone(),
            }));
        }
        // 最后才获取摘要。权威入口必读当前字节，展示预览才允许复用缓存。
        match digest(package) {
            Ok(current) if current == approval.digest => Ok(None),
            Ok(_) | Err(PackageError::StaleMetadata(_)) => Ok(Some(ApprovalGap::DigestChanged)),
            Err(error) => Err(error.into()),
        }
    }

    /// 记录一次审批。调用方必须已经把权限清单摆给用户看过——这个函数只负责落盘。
    pub fn approve(&mut self, package: &PluginPackage, now: u64) -> Result<(), RegistryError> {
        // 审批要绑定内容指纹，所以这一步必须真的读一遍包。
        let digest = package.digest_with_contents(|_, _| {})?;
        self.update(|latest| {
            latest.record_approval(package, digest, now);
            Ok(((), true))
        })
    }

    pub(super) fn approve_reviewed(
        &mut self,
        find_package: impl FnOnce() -> Option<std::sync::Arc<PluginPackage>>,
        review: &PluginApprovalReview,
        now: u64,
    ) -> Result<bool, RegistryError> {
        self.update(|latest| {
            // Resolve and verify after acquiring the same lock used to persist the approval.
            let Some(package) = find_package() else {
                return Ok((false, false));
            };
            if review.version != package.version || review.source != package.source.as_str() {
                return Ok((false, false));
            }
            let digest = match package.digest_with_contents(|_, _| {}) {
                Ok(digest) => digest,
                Err(PackageError::StaleMetadata(_) | PackageError::Unreadable { .. }) => {
                    return Ok((false, false));
                }
                Err(error) => return Err(error.into()),
            };
            if digest != review.digest {
                return Ok((false, false));
            }
            latest.record_approval(&package, digest, now);
            Ok((true, true))
        })
    }

    fn record_approval(&mut self, package: &PluginPackage, digest: String, now: u64) {
        let entry = self.file.plugins.entry(package.id.clone()).or_default();
        for key in entry.settings.keys() {
            if setting_is_declared_secret(package, key) {
                entry.setting_secrecy.insert(key.clone(), true);
            }
        }
        entry.approval = Some(PluginApproval {
            version: package.version.clone(),
            digest,
            source: package.source.as_str().to_owned(),
            permissions: permission_names(package),
            approved_at: now,
        });
    }

    /// 启用一个插件。审批不齐时拒绝并说明差在哪，不静默放行。
    pub fn set_enabled(
        &mut self,
        package: &PluginPackage,
        enabled: bool,
    ) -> Result<Result<(), ApprovalGap>, RegistryError> {
        self.update(|latest| {
            if enabled && let Some(gap) = latest.approval_gap(package)? {
                return Ok((Err(gap), false));
            }
            let entry = latest.file.plugins.entry(package.id.clone()).or_default();
            if entry.enabled != enabled {
                entry.activation = enabled.then(uuid::Uuid::new_v4);
            }
            entry.enabled = enabled;
            Ok((Ok(()), true))
        })
    }

    pub fn set_pinned_order(
        &mut self,
        plugin_id: &str,
        order: Option<u32>,
    ) -> Result<(), RegistryError> {
        self.update(|latest| {
            let entry = latest.file.plugins.entry(plugin_id.to_owned()).or_default();
            entry.pinned_order = order;
            Ok(((), true))
        })
    }

    /// 记下用户对左栏入口的选择。`None` 是撤销选择，回到清单的默认值。
    pub fn set_show_in_rail(
        &mut self,
        plugin_id: &str,
        visible: Option<bool>,
    ) -> Result<(), RegistryError> {
        self.update(|latest| {
            let entry = latest.file.plugins.entry(plugin_id.to_owned()).or_default();
            entry.show_in_rail = visible;
            Ok(((), true))
        })
    }

    /// Older registries did not record the type at the time a value was saved.
    /// Keep those values available to MCP, but do not display them in settings.
    pub fn setting_is_secret(&self, package: &PluginPackage, key: &str) -> bool {
        setting_is_declared_secret(package, key)
            || self.state(&package.id).is_some_and(|entry| {
                entry.settings.contains_key(key)
                    && entry.setting_secrecy.get(key).copied().unwrap_or(true)
            })
    }

    pub fn set_setting(
        &mut self,
        package: &PluginPackage,
        key: &str,
        value: Option<&str>,
    ) -> Result<(), RegistryError> {
        self.update(|latest| {
            let secret = latest.setting_is_secret(package, key);
            let entry = latest.file.plugins.entry(package.id.clone()).or_default();
            if let Some(value) = value {
                entry.settings.insert(key.to_owned(), value.to_owned());
                entry.setting_secrecy.insert(key.to_owned(), secret);
            } else {
                entry.settings.remove(key);
                entry.setting_secrecy.remove(key);
            }
            Ok(((), true))
        })
    }

    /// 卸载：移除该插件的全部授权与运行状态。包文件由调用方删除。
    pub fn forget(&mut self, plugin_id: &str) -> Result<(), RegistryError> {
        self.update(|latest| Ok(((), latest.file.plugins.remove(plugin_id).is_some())))
    }

    fn update<T>(
        &mut self,
        apply: impl FnOnce(&mut Self) -> Result<(T, bool), RegistryError>,
    ) -> Result<T, RegistryError> {
        let _lock = lock_state_file(&self.path).map_err(|error| RegistryError::Write {
            path: self.path.display().to_string(),
            reason: format!("cannot lock registry: {error}"),
        })?;
        let mut latest = Self::load(&self.path)?;
        let (result, changed) = apply(&mut latest)?;
        if changed {
            latest.persist()?;
        }
        self.file = latest.file;
        Ok(result)
    }

    fn persist(&self) -> Result<(), RegistryError> {
        debug_assert_eq!(self.file.version, STATE_VERSION);
        let source =
            serde_json::to_string_pretty(&self.file).map_err(|error| RegistryError::Write {
                path: self.path.display().to_string(),
                reason: error.to_string(),
            })?;
        // The sidecar lock stays on the same inode while the registry is replaced atomically.
        let temporary = self.path.with_extension("json.tmp");
        super::gateway::write_private(&temporary, source.as_bytes()).map_err(|error| {
            RegistryError::Write {
                path: temporary.display().to_string(),
                reason: error.to_string(),
            }
        })?;
        fs::rename(&temporary, &self.path).map_err(|error| RegistryError::Write {
            path: self.path.display().to_string(),
            reason: error.to_string(),
        })?;
        Ok(())
    }
}

fn setting_is_declared_secret(package: &PluginPackage, key: &str) -> bool {
    package.manifest.as_ref().is_some_and(|manifest| {
        manifest.settings.iter().any(|setting| {
            setting.id == key && setting.setting_type == super::manifest::SettingType::Secret
        })
    })
}

fn permission_names(package: &PluginPackage) -> Vec<String> {
    package
        .manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .permissions
                .iter()
                .map(|item| item.as_str().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// 没有 WillDeep 清单的 Codex 兼容包：按 `mcp.json` 推断它至少要什么权限，
/// 好让安装预览不至于显示"本插件不需要任何权限"这种明显不实的话。
pub fn inferred_permissions(package: &PluginPackage) -> Vec<PluginPermission> {
    if package.manifest.is_some() || package.mcp_servers.is_empty() {
        return Vec::new();
    }
    vec![PluginPermission::ProcessExecute]
}

#[cfg(unix)]
fn check_permissions(path: &Path) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path).map_err(|error| RegistryError::Read {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(RegistryError::Permissions {
            path: path.display().to_string(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_path: &Path) -> Result<(), RegistryError> {
    Ok(())
}

fn lock_state_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path.with_added_extension("lock"))?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("lock path is not a regular file"));
    }
    file.lock()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::package::{PluginSource, load_package};

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "willdeep-registry-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("scratch directory");
        root
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent directory");
        }
        fs::write(path, contents).expect("write fixture");
    }

    fn package(root: &Path, permissions: &str) -> PluginPackage {
        write(
            &root.join(".codex-plugin/plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        );
        write(
            &root.join(".willdeep-plugin/plugin.json"),
            &format!(
                r#"{{"schemaVersion":1,"permissions":{permissions},"contributes":{{
                    "pages":[{{"id":"p","runtime":"localWeb","entryPath":"ui/index.html"}}]}}}}"#
            ),
        );
        write(&root.join("ui/index.html"), "<html></html>");
        load_package(root, PluginSource::Shared).expect("package loads")
    }

    #[test]
    fn enabling_requires_an_approval_that_matches_the_package() {
        let home = scratch("approve");
        let installed = package(&home.join("pkg"), r#"["process.execute"]"#);
        let path = PluginRegistry::default_path(&home);
        let mut registry = PluginRegistry::load(&path).expect("load");

        assert_eq!(
            registry.approval_gap(&installed).expect("gap"),
            Some(ApprovalGap::NeverApproved)
        );
        assert!(matches!(
            registry.set_enabled(&installed, true).expect("write"),
            Err(ApprovalGap::NeverApproved)
        ));
        assert!(!registry.is_enabled("demo"));

        registry.approve(&installed, 1).expect("approve");
        assert_eq!(registry.approval_gap(&installed).expect("gap"), None);
        assert!(
            registry
                .set_enabled(&installed, true)
                .expect("write")
                .is_ok()
        );
        assert!(registry.is_enabled("demo"));

        let reloaded = PluginRegistry::load(&path).expect("reload");
        assert!(reloaded.is_enabled("demo"));
    }

    #[test]
    fn secret_and_unclassified_settings_stay_private_after_type_change() {
        let home = scratch("setting-secrecy");
        let root = home.join("pkg");
        package(&root, "[]");
        let manifest = root.join(".willdeep-plugin/plugin.json");
        let write_settings = |kind: &str| {
            write(
                &manifest,
                &serde_json::json!({
                    "schemaVersion": 1,
                    "contributes": { "settings": [
                        {"id":"access", "type":kind, "titleKey":"access"},
                        {"id":"imported", "type":kind, "titleKey":"imported"},
                        {"id":"theme", "type":"string", "titleKey":"theme"}
                    ]}
                })
                .to_string(),
            );
        };
        write(
            &root.join(".willdeep-plugin/locales/en.json"),
            r#"{"access":"Access", "imported":"Imported", "theme":"Theme"}"#,
        );
        write_settings("secret");
        let original = load_package(&root, PluginSource::Shared).unwrap();
        let path = PluginRegistry::default_path(&home);
        let mut registry = PluginRegistry::load(&path).unwrap();
        registry.approve(&original, 1).unwrap();
        registry
            .set_setting(&original, "access", Some("fixture-value"))
            .unwrap();
        registry
            .set_setting(&original, "theme", Some("dark"))
            .unwrap();
        registry
            .update(|latest| {
                latest
                    .file
                    .plugins
                    .get_mut("demo")
                    .unwrap()
                    .settings
                    .insert("imported".to_owned(), "legacy-value".to_owned());
                Ok(((), true))
            })
            .unwrap();

        write_settings("string");
        let updated = load_package(&root, PluginSource::Shared).unwrap();
        for approve in [false, true] {
            if approve {
                registry.approve(&updated, 2).unwrap();
            }
            let current = PluginRegistry::load(&path).unwrap();
            assert!(current.setting_is_secret(&updated, "access"));
            assert!(current.setting_is_secret(&updated, "imported"));
            assert!(!current.setting_is_secret(&updated, "theme"));
            assert_eq!(
                current.state("demo").unwrap().settings["imported"],
                "legacy-value"
            );
        }
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn stale_writers_merge_changes_without_restoring_removed_approval() {
        let home = scratch("writers");
        let installed = package(&home.join("pkg"), "[]");
        let path = PluginRegistry::default_path(&home);
        let mut web = PluginRegistry::load(&path).expect("web registry");
        let mut desktop = PluginRegistry::load(&path).expect("desktop registry");

        web.approve(&installed, 1).expect("approve");
        assert_eq!(
            desktop.set_enabled(&installed, true).expect("enable"),
            Ok(())
        );
        let activation = desktop.state("demo").unwrap().activation;
        assert!(activation.is_some());

        std::thread::scope(|threads| {
            threads.spawn(|| web.set_setting(&installed, "theme", Some("dark")).unwrap());
            threads.spawn(|| desktop.set_pinned_order("demo", Some(1)).unwrap());
        });
        let read = PluginRegistry::load(&path).expect("reopen");
        let state = read.state("demo").expect("plugin state");
        assert_eq!(state.approval.as_ref().unwrap().approved_at, 1);
        assert_eq!(state.activation, activation);
        assert_eq!(state.pinned_order, Some(1));
        assert_eq!(state.settings["theme"], "dark");

        web.forget("demo").expect("uninstall");
        assert_eq!(
            desktop
                .set_enabled(&installed, true)
                .expect("check fresh state"),
            Err(ApprovalGap::NeverApproved),
        );
        assert!(PluginRegistry::load(&path).unwrap().state("demo").is_none());
    }

    #[test]
    fn changed_content_and_new_permissions_invalidate_an_approval() {
        let home = scratch("invalidate");
        let package_root = home.join("pkg");
        let installed = package(&package_root, r#"["process.execute"]"#);
        let mut registry =
            PluginRegistry::load(&PluginRegistry::default_path(&home)).expect("load");
        registry.approve(&installed, 1).expect("approve");

        // 同版本、同来源，只改内容：这比换版本更该拦。
        write(&package_root.join("ui/index.html"), "<html>changed</html>");
        let tampered = load_package(&package_root, PluginSource::Shared).expect("reload");
        assert_eq!(
            registry.approval_gap(&tampered).expect("gap"),
            Some(ApprovalGap::DigestChanged)
        );

        // 插件加要权限时，用户该看到的理由是"它现在还想要网络访问"，
        // 不是笼统的"内容变了"——改权限必然改 digest，两者会同时成立。
        let widened = package(&package_root, r#"["process.execute","network.access"]"#);
        assert!(matches!(
            registry.approval_gap(&widened).expect("gap"),
            Some(ApprovalGap::NewPermissions(added)) if added == vec!["network.access".to_owned()]
        ));
    }

    fn approved_then_rewritten_package(name: &str) -> (PluginPackage, PluginRegistry, String) {
        let home = scratch(name);
        let installed = package(&home.join("pkg"), "[]");
        let payload = installed.root.join("ui/index.html");
        write(&payload, "one");
        let mut registry =
            PluginRegistry::load(&PluginRegistry::default_path(&home)).expect("registry");
        registry
            .approve(&installed, 1)
            .expect("approve original bytes");
        let old = installed.cached_digest().expect("warm original preview");
        let before = fs::metadata(&payload).expect("original metadata");
        assert_eq!(before.len(), "two".len() as u64);
        fs::write(&payload, "two").expect("rewrite payload only");
        fs::OpenOptions::new()
            .write(true)
            .open(&payload)
            .expect("open payload")
            .set_times(fs::FileTimes::new().set_modified(before.modified().expect("mtime")))
            .expect("restore mtime");
        let after = fs::metadata(&payload).expect("rewritten metadata");
        assert_eq!(after.len(), before.len());
        assert_eq!(
            after.modified().expect("mtime"),
            before.modified().expect("mtime")
        );
        assert_eq!(installed.cached_digest().expect("stale preview"), old);
        assert_eq!(
            registry.approval_gap_preview(&installed).expect("preview"),
            None
        );
        assert!(!registry.is_enabled(&installed.id));
        (installed, registry, old)
    }

    #[test]
    fn authoritative_approval_gap_detects_rewrite_hidden_from_preview() {
        let (installed, registry, _) = approved_then_rewritten_package("fresh-gap");
        assert_eq!(
            registry
                .approval_gap(&installed)
                .expect("current bytes checked"),
            Some(ApprovalGap::DigestChanged)
        );
    }

    #[test]
    fn enabling_rejects_rewrite_hidden_from_preview() {
        let (installed, mut registry, _) = approved_then_rewritten_package("fresh-enable");
        assert_eq!(
            registry
                .set_enabled(&installed, true)
                .expect("enable checked"),
            Err(ApprovalGap::DigestChanged)
        );
        assert!(!registry.is_enabled(&installed.id));
        let reloaded = PluginRegistry::load(&registry.path).expect("reload disabled state");
        assert!(!reloaded.is_enabled(&installed.id));
    }

    #[test]
    fn reapproval_persists_current_bytes_despite_stale_preview() {
        let (installed, mut registry, old) = approved_then_rewritten_package("fresh-reapprove");
        registry
            .approve(&installed, 2)
            .expect("approve current bytes");
        let reloaded = PluginRegistry::load(&registry.path).expect("reload approval");
        let approval = reloaded
            .state(&installed.id)
            .expect("state")
            .approval
            .as_ref()
            .expect("approval");
        assert_ne!(approval.digest, old);
        assert_eq!(approval.digest, installed.digest().expect("current digest"));
        assert_eq!(approval.approved_at, 2);
        assert_eq!(
            reloaded.approval_gap(&installed).expect("current approval"),
            None
        );
    }

    #[test]
    fn never_approved_checks_do_not_read_payload() {
        let home = scratch("lazy-never-approved");
        let installed = package(&home.join("pkg"), "[]");
        fs::File::create(installed.root.join("ui/huge.bin"))
            .expect("payload")
            .set_len(super::super::package::MAX_FILE_BYTES + 1)
            .expect("sparse oversized payload");
        let registry =
            PluginRegistry::load(&PluginRegistry::default_path(&home)).expect("registry");
        assert_eq!(
            registry
                .approval_gap(&installed)
                .expect("lazy authoritative check"),
            Some(ApprovalGap::NeverApproved)
        );
        assert_eq!(
            registry
                .approval_gap_preview(&installed)
                .expect("lazy preview"),
            Some(ApprovalGap::NeverApproved)
        );
        assert!(
            installed.digest().is_err(),
            "reading payload would reject it"
        );
    }

    #[test]
    fn approval_preview_and_authoritative_check_keep_short_circuit_order() {
        let home = scratch("gap-order");
        let mut installed = package(&home.join("pkg"), r#"["process.execute"]"#);
        let mut registry =
            PluginRegistry::load(&PluginRegistry::default_path(&home)).expect("registry");
        registry.approve(&installed, 1).expect("approve original");
        let approval = registry
            .file
            .plugins
            .get_mut(&installed.id)
            .expect("state")
            .approval
            .as_mut()
            .expect("approval");
        approval.version = "0.9.0".to_owned();
        approval.permissions.clear();
        approval.source = "draft".to_owned();
        // 三项差异同时存在；超限 payload 确保这两个公开入口都在摘要前短路。
        fs::File::create(installed.root.join("ui/huge.bin"))
            .expect("payload")
            .set_len(super::super::package::MAX_FILE_BYTES + 1)
            .expect("sparse oversized payload");
        for expected in [
            ApprovalGap::VersionChanged {
                approved: "0.9.0".to_owned(),
            },
            ApprovalGap::NewPermissions(vec!["process.execute".to_owned()]),
            ApprovalGap::SourceChanged {
                approved: "draft".to_owned(),
            },
        ] {
            assert_eq!(
                registry.approval_gap(&installed).expect("short circuit"),
                Some(expected.clone())
            );
            assert_eq!(
                registry
                    .approval_gap_preview(&installed)
                    .expect("short preview"),
                Some(expected.clone())
            );
            match expected {
                ApprovalGap::VersionChanged { .. } => installed.version = "0.9.0".to_owned(),
                ApprovalGap::NewPermissions(_) => installed
                    .manifest
                    .as_mut()
                    .expect("manifest")
                    .permissions
                    .clear(),
                ApprovalGap::SourceChanged { .. } => {}
                _ => unreachable!("only metadata gaps tested"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_world_readable_registry_is_refused_outright() {
        use std::os::unix::fs::PermissionsExt;
        let home = scratch("permissions");
        let installed = package(&home.join("pkg"), "[]");
        let path = PluginRegistry::default_path(&home);
        let mut registry = PluginRegistry::load(&path).expect("load");
        registry.approve(&installed, 1).expect("approve");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "registry must be written as 0600");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen");
        assert!(matches!(
            PluginRegistry::load(&path),
            Err(RegistryError::Permissions { .. })
        ));
    }

    #[test]
    fn codex_only_packages_are_shown_as_needing_process_execution() {
        let home = scratch("inferred");
        let root = home.join("pkg");
        write(
            &root.join(".codex-plugin/plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        );
        write(
            &root.join("mcp.json"),
            r#"{"mcpServers":{"demo":{"command":"/usr/bin/ruby","args":["x.rb"]}}}"#,
        );
        let installed = load_package(&root, PluginSource::Shared).expect("loads");
        assert!(installed.manifest.is_none());
        assert_eq!(
            inferred_permissions(&installed),
            vec![PluginPermission::ProcessExecute]
        );
    }
}
