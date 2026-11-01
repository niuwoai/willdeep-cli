//! Install and inspect skills on disk.
//!
//! A skill is a directory holding a `SKILL.md`. Installing one means copying
//! such a directory into `~/.willdeep/skills/<name>`, which is the first user
//! root [`willdeep_core::SkillCatalog`] scans. The copy comes from a local
//! directory or from a public GitHub repository; both paths share the same
//! checks, so a hostile archive cannot land a skill the local path would refuse.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const MAX_DOWNLOAD_BYTES: u64 = 32 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SKILL_BYTES: u64 = 8 * 1024 * 1024;

/// Where skills the user installs end up. The catalog scans this directory, so
/// nothing else has to be configured for a newly installed skill to show up.
pub fn user_skills_dir(home: &Path) -> PathBuf {
    home.join("skills")
}

/// One installed skill, as the configuration page lists it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct InstalledSkill {
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub tier: Option<String>,
    /// The skill directory. Editing writes `SKILL.md` inside it.
    pub directory: PathBuf,
    /// True when the directory sits under the user's own skills directory, so
    /// the page may edit or remove it. A skill discovered from the workspace or
    /// from an extra root is listed but not writable here.
    pub editable: bool,
}

/// Every skill the catalog would find for one workspace, including the extra
/// roots from `[skills].roots`.
pub fn list(workspace: &Path, home: &Path, extra_roots: &[PathBuf]) -> Vec<InstalledSkill> {
    let catalog = willdeep_core::SkillCatalog::discover(workspace, extra_roots);
    let own = user_skills_dir(home);
    catalog
        .list()
        .iter()
        .map(|skill| {
            let directory = skill.path.parent().unwrap_or(&skill.path).to_path_buf();
            InstalledSkill {
                identifier: skill.identifier.clone(),
                name: skill.name.clone(),
                description: skill.description.clone(),
                tier: skill.tier.map(|tier| tier.as_str().to_owned()),
                editable: directory.starts_with(&own),
                directory,
            }
        })
        .collect()
}

/// Read a skill's `SKILL.md`. The path must be a skill directory the catalog
/// could have found; an arbitrary path is refused.
pub fn read(workspace: &Path, home: &Path, extra_roots: &[PathBuf], directory: &Path) -> Result<String> {
    let skill = owned(workspace, home, extra_roots, directory)?;
    std::fs::read_to_string(skill.directory.join("SKILL.md"))
        .with_context(|| format!("read {}", skill.directory.display()))
}

/// Replace a skill's `SKILL.md`. Only skills under the user's own directory are
/// writable; a workspace skill belongs to the project, not to this page.
pub fn write(home: &Path, directory: &Path, content: &str) -> Result<()> {
    ensure_editable(home, directory)?;
    let path = directory.join("SKILL.md");
    if !path.is_file() {
        bail!("{} has no SKILL.md", directory.display());
    }
    std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

/// Create a new skill under the user's directory. The name becomes the
/// directory name, so it has to be a single safe path segment.
pub fn create(home: &Path, name: &str, content: &str) -> Result<PathBuf> {
    let name = skill_name(name)?;
    let directory = user_skills_dir(home).join(&name);
    if directory.exists() {
        bail!("a skill named {name} already exists");
    }
    std::fs::create_dir_all(&directory)?;
    if let Err(error) = std::fs::write(directory.join("SKILL.md"), content) {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error).context("write SKILL.md");
    }
    Ok(directory)
}

/// Remove a skill the user installed. Refuses anything outside their directory.
pub fn remove(home: &Path, directory: &Path) -> Result<()> {
    ensure_editable(home, directory)?;
    if !directory.join("SKILL.md").is_file() {
        bail!("{} is not a skill directory", directory.display());
    }
    std::fs::remove_dir_all(directory)
        .with_context(|| format!("remove {}", directory.display()))
}

fn owned(
    workspace: &Path,
    home: &Path,
    extra_roots: &[PathBuf],
    directory: &Path,
) -> Result<InstalledSkill> {
    let canonical = directory
        .canonicalize()
        .with_context(|| format!("open {}", directory.display()))?;
    list(workspace, home, extra_roots)
        .into_iter()
        .find(|skill| skill.directory == canonical)
        .with_context(|| format!("{} is not an installed skill", directory.display()))
}

fn ensure_editable(home: &Path, directory: &Path) -> Result<()> {
    let canonical = directory
        .canonicalize()
        .with_context(|| format!("open {}", directory.display()))?;
    let own = user_skills_dir(home);
    let own = own.canonicalize().unwrap_or(own);
    if !canonical.starts_with(&own) {
        bail!(
            "{} is outside the user skill directory; edit it in its own project",
            canonical.display()
        );
    }
    Ok(())
}

fn skill_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("skill name must be 1-64 characters of letters, digits, '-' or '_'");
    }
    Ok(name.to_owned())
}

/// A GitHub location a skill can be installed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitSource {
    pub owner: String,
    pub repo: String,
    pub reference: String,
    /// Directory inside the repository. Empty installs a skill repository whose
    /// skills sit at the top level.
    pub subdir: String,
}

impl GitSource {
    fn archive_url(&self) -> String {
        format!(
            "https://codeload.github.com/{}/{}/zip/refs/heads/{}",
            self.owner, self.repo, self.reference
        )
    }
}

/// Parse `owner/repo`, a github.com URL, or a URL that points at a branch and a
/// subdirectory (`.../tree/<ref>/<path>`).
pub fn parse_git_source(input: &str) -> Result<GitSource> {
    let input = input.trim().trim_end_matches('/');
    if input.is_empty() {
        bail!("empty address");
    }
    let (host, path) = match input.split_once("://") {
        Some((scheme, rest)) => {
            if !scheme.eq_ignore_ascii_case("https") {
                bail!("only https:// GitHub addresses are accepted");
            }
            let (host, path) = rest.split_once('/').context("address has no repository path")?;
            (host.to_owned(), path.to_owned())
        }
        None => ("github.com".to_owned(), input.to_owned()),
    };
    if host != "github.com" && host != "www.github.com" {
        bail!("only github.com repositories can be installed, got {host}");
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() < 2 {
        bail!("address needs an owner and a repository");
    }
    let owner = git_segment(parts[0], "owner")?;
    let repo = git_segment(parts[1].trim_end_matches(".git"), "repository")?;
    let mut reference = String::from("main");
    let mut subdir = String::new();
    if let Some(kind) = parts.get(2).copied() {
        if kind != "tree" && kind != "blob" {
            bail!("unsupported GitHub URL shape ({kind}); use owner/repo or a /tree/<branch>/<path> URL");
        }
        reference = parts
            .get(3)
            .context("address names no branch")?
            .to_string();
        if reference.is_empty() || reference.contains("..") {
            bail!("invalid branch name");
        }
        subdir = parts[4..].join("/");
    }
    if subdir.split('/').any(|part| part == ".." || part.is_empty()) {
        bail!("invalid subdirectory");
    }
    Ok(GitSource { owner, repo, reference, subdir })
}

fn git_segment(value: &str, label: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        bail!("invalid GitHub {label}: {value}");
    }
    Ok(value.to_owned())
}

/// Download a public repository and copy the skills it contains into the user's
/// skill directory. Returns the skills that were installed.
///
/// An existing skill of the same name is left untouched and reported as an
/// error rather than overwritten: installing must not destroy a skill the user
/// has edited.
pub fn install_from_git(home: &Path, source: &GitSource) -> Result<Vec<String>> {
    let staging = std::env::temp_dir().join(format!("willdeep-skill-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&staging)?;
    let result = (|| {
        let archive = staging.join("repo.zip");
        download(&source.archive_url(), &archive)?;
        let extracted = staging.join("src");
        extract_zip(&archive, &extracted)?;
        let root = find_repo_root(&extracted, source)?;
        let skills = find_skills(&root, &source.subdir)?;
        if skills.is_empty() {
            bail!(
                "no SKILL.md found under {}/{}",
                source.repo,
                if source.subdir.is_empty() { "(repository root)" } else { &source.subdir }
            );
        }
        let mut installed = Vec::new();
        for skill in &skills {
            install_directory(home, skill)?;
            installed.push(
                skill
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        Ok(installed)
    })();
    if let Err(error) = std::fs::remove_dir_all(&staging) {
        eprintln!("warning: skill install staging cleanup failed: {error}");
    }
    result
}

fn download(url: &str, destination: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            "60",
            "--max-filesize",
            &MAX_DOWNLOAD_BYTES.to_string(),
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .status()
        .context("run curl to download the repository")?;
    if !status.success() {
        bail!("download failed (HTTP {}). The repository must be public and the branch must exist.", status.code().unwrap_or(1));
    }
    let size = std::fs::metadata(destination).map(|m| m.len()).unwrap_or(0);
    if size == 0 || size > MAX_DOWNLOAD_BYTES {
        bail!("downloaded archive is {size} bytes, expected between 1 and {MAX_DOWNLOAD_BYTES}");
    }
    Ok(())
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    // Names are checked before anything is written: a `..` or an absolute path
    // in the archive aborts the install instead of landing outside the staging
    // directory. `ditto` is the macOS tool; `unzip` covers Linux.
    let names = zip_entry_names(archive)?;
    if names.is_empty() {
        bail!("archive is empty");
    }
    for name in &names {
        safe_zip_path(name).with_context(|| format!("zip entry {name}"))?;
    }
    let status = if cfg!(target_os = "macos") {
        Command::new("ditto").args(["-xk"]).arg(archive).arg(destination).status()
    } else {
        Command::new("unzip")
            .args(["-q", "-o"])
            .arg(archive)
            .arg("-d")
            .arg(destination)
            .status()
    }
    .context("extract the downloaded archive")?;
    if !status.success() {
        bail!("extracting the archive failed");
    }
    let extracted = directory_size(destination)?;
    if extracted > MAX_EXTRACTED_BYTES {
        bail!("archive expands to {extracted} bytes, past the {MAX_EXTRACTED_BYTES} byte limit");
    }
    Ok(())
}

fn zip_entry_names(archive: &Path) -> Result<Vec<String>> {
    let output = if cfg!(target_os = "macos") {
        Command::new("zipinfo").args(["-1"]).arg(archive).output()
    } else {
        Command::new("unzip").args(["-Z1"]).arg(archive).output()
    }
    .context("list archive entries")?;
    if !output.status.success() {
        bail!("cannot list archive entries");
    }
    let text = String::from_utf8(output.stdout).context("archive entry names are not UTF-8")?;
    Ok(text.lines().map(|line| line.trim().to_owned()).filter(|line| !line.is_empty()).collect())
}

/// Zip entries are untrusted. A `..`, an absolute path, or a backslash that
/// would change meaning on Windows is rejected outright.
fn safe_zip_path(name: &str) -> Result<PathBuf> {
    if name.is_empty() {
        bail!("empty entry name");
    }
    let mut path = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.contains('\\') || part.contains(':') {
                    bail!("entry name contains a path separator");
                }
                path.push(part.as_ref());
            }
            Component::CurDir => {}
            _ => bail!("entry is absolute or climbs with '..'"),
        }
    }
    if path.as_os_str().is_empty() {
        bail!("entry has no usable path");
    }
    Ok(path)
}

fn find_repo_root(extracted: &Path, source: &GitSource) -> Result<PathBuf> {
    let prefix = format!("{}-{}-", source.owner, source.repo);
    for entry in std::fs::read_dir(extracted).context("read extracted archive")? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && entry.file_name().to_string_lossy().starts_with(&prefix)
        {
            return Ok(entry.path());
        }
    }
    // GitHub names the top directory `<repo>-<ref>`, which does not always
    // match the owner. Fall back to the only directory present.
    let dirs: Vec<PathBuf> = std::fs::read_dir(extracted)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    match dirs.as_slice() {
        [only] => Ok(only.clone()),
        _ => bail!("archive does not contain exactly one repository directory"),
    }
}

fn find_skills(root: &Path, subdir: &str) -> Result<Vec<PathBuf>> {
    let base = if subdir.is_empty() { root.to_path_buf() } else { root.join(subdir) };
    if !base.is_dir() {
        bail!("{} does not exist in the repository", base.display());
    }
    let mut skills = Vec::new();
    if base.join("SKILL.md").is_file() {
        skills.push(base);
        return Ok(skills);
    }
    for entry in std::fs::read_dir(&base)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("SKILL.md").is_file() {
            skills.push(entry.path());
        }
    }
    skills.sort();
    Ok(skills)
}

fn install_directory(home: &Path, source: &Path) -> Result<()> {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .context("skill directory has no name")?;
    let name = skill_name(&name)?;
    let destination = user_skills_dir(home).join(&name);
    if destination.exists() {
        bail!("skill {name} is already installed at {}", destination.display());
    }
    let size = directory_size(source)?;
    if size > MAX_SKILL_BYTES {
        bail!("skill {name} is {size} bytes, over the {MAX_SKILL_BYTES} byte limit");
    }
    std::fs::create_dir_all(&destination)?;
    if let Err(error) = copy_tree(source, &destination) {
        let _ = std::fs::remove_dir_all(&destination);
        return Err(error);
    }
    Ok(())
}

fn directory_size(root: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for entry in walk(root)? {
        total += entry.metadata().map(|m| m.len()).unwrap_or(0);
    }
    Ok(total)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    for entry in walk(source)? {
        let relative = entry.strip_prefix(source).unwrap_or(&entry);
        let target = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&entry, &target)
                .with_context(|| format!("copy {}", entry.display()))?;
        }
    }
    Ok(())
}

/// Files and directories under `root`, skipping symlinks. A skill that points
/// outside its own directory is not something this installer should reproduce.
fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.symlink_metadata()?.file_type().is_symlink() {
                continue;
            }
            out.push(path.clone());
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let root = std::env::temp_dir().join(format!("skill-lib-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn github_addresses_parse_and_strangers_do_not() {
        let plain = parse_git_source("niuwoai/agentrules").unwrap();
        assert_eq!(plain.owner, "niuwoai");
        assert_eq!(plain.reference, "main");
        assert!(plain.subdir.is_empty());

        let nested = parse_git_source(
            "https://github.com/acme/skills/tree/v1.2.0/skills/reviewer",
        )
        .unwrap();
        assert_eq!(nested.reference, "v1.2.0");
        assert_eq!(nested.subdir, "skills/reviewer");

        assert!(parse_git_source("https://gitlab.com/acme/skills").is_err());
        assert!(parse_git_source("https://github.com/acme/skills/tree/main/../etc").is_err());
        assert!(parse_git_source("http://github.com/acme/skills").is_err());
    }

    #[test]
    fn a_zip_entry_that_climbs_out_is_rejected() {
        assert!(safe_zip_path("repo-main/SKILL.md").is_ok());
        assert!(safe_zip_path("../SKILL.md").is_err());
        assert!(safe_zip_path("/etc/passwd").is_err());
        assert!(safe_zip_path("dir/../../outside").is_err());
    }

    #[test]
    fn installing_copies_the_skill_and_refuses_to_overwrite() {
        let root = scratch();
        let source = root.join("incoming/reviewer");
        std::fs::create_dir_all(source.join("scripts")).unwrap();
        std::fs::write(source.join("SKILL.md"), "---\nname: Reviewer\ndescription: Review\n---\n").unwrap();
        std::fs::write(source.join("scripts/run.sh"), "echo hi").unwrap();
        let home = root.join("home");

        install_directory(&home, &source).unwrap();
        let installed = home.join("skills/reviewer/SKILL.md");
        assert!(installed.is_file());
        assert!(home.join("skills/reviewer/scripts/run.sh").is_file());

        let again = install_directory(&home, &source);
        assert!(again.is_err(), "an existing skill must not be overwritten");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn editing_is_confined_to_the_user_directory() {
        let root = scratch();
        let home = root.join("home");
        let workspace = root.join("work");
        let own = home.join("skills/mine");
        let foreign = workspace.join(".willdeep/skills/theirs");
        for directory in [&own, &foreign] {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join("SKILL.md"), "---\nname: x\ndescription: d\n---\n").unwrap();
        }

        write(&home, &own, "edited").unwrap();
        assert_eq!(std::fs::read_to_string(own.join("SKILL.md")).unwrap(), "edited");
        assert!(write(&home, &foreign, "nope").is_err());

        let listed = list(&workspace, &home, &[]);
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|skill| skill.editable && skill.identifier == "mine"));
        assert!(listed.iter().any(|skill| !skill.editable && skill.identifier == "theirs"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
