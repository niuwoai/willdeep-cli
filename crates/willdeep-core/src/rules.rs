//! Global rules: the long-lived instructions that ride the cached prompt prefix.
//!
//! A rule is one file, or one directory of `*.md` files joined in file-name
//! order. The configured list lives in `[rules].files`; a configuration that
//! has never named one still reads `~/.willdeep/CLAUDE.md`, which is what every
//! earlier version did.
//!
//! The rendered text goes into the stable part of the system prompt, ahead of
//! the cache breakpoint. It changes only when the user edits a rule file, so a
//! change there is exactly when the provider cache should miss.

use std::path::{Component, Path, PathBuf};

/// One file, read in full. A rule library is meant to be a handful of short
/// documents; anything larger belongs behind a skill the model reads on demand.
const MAX_RULE_FILE_BYTES: u64 = 256 * 1024;
/// Everything injected together. Past this the tail is dropped and the prompt
/// says so, rather than silently feeding the model half a rule.
const MAX_RULES_TOTAL_BYTES: usize = 512 * 1024;

/// Built-in copy of the compact niuwoai/agentrules set. The header of the file
/// names the upstream version it was cut from.
pub const BUNDLED_AGENTRULES: &str = include_str!("../assets/rules/agentrules.md");

/// Where the bundled rules land, and the directory `[rules].files` entries are
/// relative to when they are not absolute.
pub fn rules_dir() -> Option<PathBuf> {
    willdeep_home().map(|home| home.join("rules"))
}

/// The single file older versions read. Still the fallback when no `[rules]`
/// section has ever been written.
pub fn legacy_rules_file() -> Option<PathBuf> {
    willdeep_home().map(|home| home.join("CLAUDE.md"))
}

fn willdeep_home() -> Option<PathBuf> {
    std::env::var_os("WILLDEEP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".willdeep")))
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".willdeep"))
        })
}

/// One configured rule and what happened when it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleSource {
    /// The path as the user configured it, before expansion.
    pub configured: String,
    pub resolved: PathBuf,
    pub status: RuleStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleStatus {
    /// Readable and within the size limits. `bytes` is what was injected.
    Loaded { bytes: usize, files: usize },
    /// Not there. Not an error: a rule the user removed should not block a run.
    Missing,
    /// There, but not usable. The reason is reported in the prompt so the model
    /// does not pretend it read the rule.
    Unreadable { reason: String },
    /// Dropped because the total budget was already spent.
    Skipped { reason: String },
}

/// Everything the prompt needs from the configured rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedRules {
    pub text: String,
    pub sources: Vec<RuleSource>,
}

/// Read the rules the configuration names, falling back to the legacy file when
/// the configuration names none.
///
/// `configured` is `[rules].files` exactly as parsed. `None` means the section
/// was never written, which is the legacy case; `Some([])` means the user
/// cleared the list on purpose and wants no global rules.
pub fn load(configured: Option<&[String]>) -> LoadedRules {
    let paths = match configured {
        Some(files) => files.to_vec(),
        None => match legacy_rules_file() {
            Some(path) if path.is_file() => vec![path.display().to_string()],
            _ => {
                return LoadedRules {
                    text: String::new(),
                    sources: Vec::new(),
                };
            }
        },
    };
    render(&paths)
}

/// Read one explicit list of rule paths. `~` expands, relative paths resolve
/// against the rules directory, and a directory contributes its `*.md` files in
/// name order.
pub fn render(configured_paths: &[String]) -> LoadedRules {
    let mut sources = Vec::new();
    let mut bodies = Vec::new();
    let mut used = 0_usize;
    for configured in configured_paths {
        let configured = configured.trim();
        if configured.is_empty() {
            continue;
        }
        let resolved = resolve(configured);
        if used >= MAX_RULES_TOTAL_BYTES {
            sources.push(RuleSource {
                configured: configured.to_owned(),
                resolved,
                status: RuleStatus::Skipped {
                    reason: format!(
                        "the {MAX_RULES_TOTAL_BYTES} byte rule budget is already spent"
                    ),
                },
            });
            continue;
        }
        match read_rule(&resolved, MAX_RULES_TOTAL_BYTES - used) {
            Ok(Read::Missing) => sources.push(RuleSource {
                configured: configured.to_owned(),
                resolved,
                status: RuleStatus::Missing,
            }),
            Ok(Read::Body { text, bytes, files }) => {
                used += bytes;
                bodies.push((resolved.display().to_string(), text));
                sources.push(RuleSource {
                    configured: configured.to_owned(),
                    resolved,
                    status: RuleStatus::Loaded { bytes, files },
                });
            }
            Err(reason) => sources.push(RuleSource {
                configured: configured.to_owned(),
                resolved,
                status: RuleStatus::Unreadable { reason },
            }),
        }
    }
    LoadedRules {
        text: compose(&bodies, &sources),
        sources,
    }
}

fn compose(bodies: &[(String, String)], sources: &[RuleSource]) -> String {
    if bodies.is_empty()
        && sources
            .iter()
            .all(|source| source.status == RuleStatus::Missing)
    {
        return String::new();
    }
    let mut sections = Vec::new();
    if !bodies.is_empty() {
        sections.push(
            "Global rules. Each block is one configured rule, in the order the user configured them. Later rules are more specific than earlier ones. Project and directory rules are more specific than these, and the user's latest message is the most specific of all."
                .to_owned(),
        );
        for (label, text) in bodies {
            sections.push(format!(
                "Rule source: {label} ({} lines, {} characters; complete)\n{text}",
                text.lines().count(),
                text.chars().count()
            ));
        }
    }
    let notes: Vec<String> = sources
        .iter()
        .filter_map(|source| match &source.status {
            RuleStatus::Unreadable { reason } => Some(format!(
                "Rule {} was not loaded: {reason}",
                source.resolved.display()
            )),
            RuleStatus::Skipped { reason } => Some(format!(
                "Rule {} was skipped: {reason}",
                source.resolved.display()
            )),
            RuleStatus::Loaded { .. } | RuleStatus::Missing => None,
        })
        .collect();
    if !notes.is_empty() {
        sections.push(notes.join("\n"));
    }
    sections.join("\n\n")
}

enum Read {
    Missing,
    Body {
        text: String,
        bytes: usize,
        files: usize,
    },
}

fn read_rule(path: &Path, remaining: usize) -> Result<Read, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Read::Missing),
        Err(error) => return Err(error.to_string()),
    };
    let files = if metadata.is_dir() {
        markdown_files(path)?
    } else if metadata.is_file() {
        vec![path.to_path_buf()]
    } else {
        return Err("not a file or directory".to_owned());
    };
    if files.is_empty() {
        return Ok(Read::Missing);
    }
    let mut parts = Vec::new();
    let mut bytes = 0_usize;
    for file in &files {
        let content = read_bounded(file)?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            continue;
        }
        if bytes + trimmed.len() > remaining {
            return Err(format!(
                "including it would pass the {MAX_RULES_TOTAL_BYTES} byte total"
            ));
        }
        bytes += trimmed.len();
        let body = if files.len() == 1 {
            trimmed.to_owned()
        } else {
            format!(
                "# {}\n{trimmed}",
                file.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        };
        parts.push(body);
    }
    if parts.is_empty() {
        return Ok(Read::Missing);
    }
    Ok(Read::Body {
        text: parts.join("\n\n"),
        bytes,
        files: parts.len(),
    })
}

fn markdown_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let is_markdown = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
        if is_markdown && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn read_bounded(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut buffer = Vec::new();
    file.by_ref()
        .take(MAX_RULE_FILE_BYTES + 1)
        .read_to_end(&mut buffer)
        .map_err(|error| error.to_string())?;
    if buffer.len() as u64 > MAX_RULE_FILE_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_RULE_FILE_BYTES} byte limit; split it, nothing was truncated",
            path.display()
        ));
    }
    String::from_utf8(buffer).map_err(|_| format!("{} is not valid UTF-8", path.display()))
}

fn resolve(configured: &str) -> PathBuf {
    let expanded = expand_tilde(configured);
    let path = PathBuf::from(&expanded);
    if path.is_absolute() {
        return path;
    }
    // A relative rule path is relative to the rules directory, not to whatever
    // directory the process happens to have been started in.
    rules_dir().map(|dir| dir.join(&path)).unwrap_or(path)
}

fn expand_tilde(value: &str) -> String {
    let Some(rest) = value.strip_prefix('~') else {
        return value.to_owned();
    };
    if !rest.is_empty() && !rest.starts_with('/') && !rest.starts_with('\\') {
        return value.to_owned();
    }
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) => Path::new(&home)
            .join(rest.trim_start_matches(['/', '\\']))
            .display()
            .to_string(),
        None => value.to_owned(),
    }
}

/// Write the bundled rule set the first time, and only the first time.
///
/// Returns the path written, or `None` when the rules directory already exists
/// — the user has rules of their own, or already deleted the bundled ones, and
/// neither should be put back.
pub fn seed_bundled_rules() -> std::io::Result<Option<PathBuf>> {
    let Some(directory) = rules_dir() else {
        return Ok(None);
    };
    if directory.exists() {
        return Ok(None);
    }
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("agentrules.md");
    if let Err(error) = std::fs::write(&path, BUNDLED_AGENTRULES) {
        let _ = std::fs::remove_dir(&directory);
        return Err(error);
    }
    Ok(Some(path))
}

/// The path a rule editor may write. Absolute paths and `~` are allowed;
/// anything that climbs out with `..` is refused, because the page takes this
/// path from the browser.
pub fn editable_rule_path(configured: &str) -> Result<PathBuf, String> {
    if configured.trim().is_empty() {
        return Err("规则路径不能为空".to_owned());
    }
    if configured.split(['/', '\\']).any(|part| part == "..") {
        return Err("规则路径不能包含 ..".to_owned());
    }
    let path = resolve(configured.trim());
    if path
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("规则路径不能包含 ..".to_owned());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let root = std::env::temp_dir().join(format!("willdeep-rules-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_directory_rule_joins_its_markdown_in_name_order() {
        let root = scratch();
        std::fs::write(root.join("b.md"), "second").unwrap();
        std::fs::write(root.join("a.md"), "first").unwrap();
        std::fs::write(root.join("notes.txt"), "ignored").unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/c.md"), "too deep").unwrap();

        let loaded = render(&[root.display().to_string()]);

        assert_eq!(loaded.sources.len(), 1);
        assert!(matches!(
            loaded.sources[0].status,
            RuleStatus::Loaded { files: 2, .. }
        ));
        let first = loaded.text.find("first").unwrap();
        let second = loaded.text.find("second").unwrap();
        assert!(first < second, "files must come in name order");
        assert!(!loaded.text.contains("ignored"));
        assert!(
            !loaded.text.contains("too deep"),
            "subdirectories are not scanned"
        );
        assert!(loaded.text.contains("Later rules are more specific"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_rule_is_skipped_and_an_unreadable_one_is_reported() {
        let root = scratch();
        let missing = root.join("gone.md");
        std::fs::write(root.join("bad.md"), [0xff, 0xfe]).unwrap();

        let loaded = render(&[
            missing.display().to_string(),
            root.join("bad.md").display().to_string(),
        ]);

        assert!(matches!(loaded.sources[0].status, RuleStatus::Missing));
        assert!(matches!(
            loaded.sources[1].status,
            RuleStatus::Unreadable { .. }
        ));
        assert!(loaded.text.contains("not valid UTF-8"));
        assert!(
            !loaded.text.contains("Global rules."),
            "no readable rule, no preamble"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_oversized_file_is_refused_whole_rather_than_cut() {
        let root = scratch();
        let path = root.join("big.md");
        std::fs::write(&path, "x".repeat(MAX_RULE_FILE_BYTES as usize + 1)).unwrap();

        let loaded = render(&[path.display().to_string()]);

        assert!(matches!(
            loaded.sources[0].status,
            RuleStatus::Unreadable { .. }
        ));
        assert!(loaded.text.contains("exceeds"));
        assert!(
            !loaded.text.contains(&"x".repeat(100)),
            "nothing was injected"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_configured_rules_and_no_legacy_file_injects_nothing() {
        let loaded = load(Some(&[]));
        assert!(loaded.text.is_empty());
        assert!(loaded.sources.is_empty());
    }

    #[test]
    fn the_bundled_rules_seed_once_and_never_overwrite() {
        let home = scratch();
        // SAFETY: the test process is single threaded here, and the helper
        // reads the variable back before any other thread could.
        unsafe { std::env::set_var("WILLDEEP_HOME", &home) };

        let written = seed_bundled_rules().unwrap().unwrap();
        assert_eq!(written, home.join("rules/agentrules.md"));
        assert!(written.is_file());
        std::fs::write(&written, "user edited").unwrap();

        assert!(
            seed_bundled_rules().unwrap().is_none(),
            "an existing dir is left alone"
        );
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "user edited");

        unsafe { std::env::remove_var("WILLDEEP_HOME") };
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_rule_path_may_not_climb_out() {
        assert!(editable_rule_path("../secret.md").is_err());
        assert!(editable_rule_path("rules/../../etc/passwd").is_err());
        assert!(editable_rule_path("team.md").is_ok());
    }
}
