//! Narrow full-access exception for literal, bounded cleanup commands.
//! Other modes and the general destructive classifier keep their old policy.

use std::path::{Component, Path, PathBuf};

use super::{CommandSafety, classify, split_segments, tokenize};

const TEMP_ROOTS: &[&str] = &["/tmp", "/private/tmp", "/var/tmp"];
const MAX_DIRECTORY_CANDIDATES: usize = 32;
const OBSERVATION_HEADS: &[&str] = &[
    "echo", "printf", "pwd", "ls", "cat", "head", "tail", "wc", "true", "false",
];
const RM_LONG_OPTIONS: &[&str] = &["--force", "--recursive", "--dir", "--verbose"];

/// This deliberately accepts a small shell grammar. Expansion, wrappers,
/// background jobs, and ambiguous control flow must still ask the operator.
pub(crate) fn allows_full_access_removal(command: &str, workspace: &Path) -> bool {
    if cfg!(windows) || has_dynamic_syntax(command) {
        return false;
    }
    let Some(segments) = split_segments(command) else {
        return false;
    };
    let Some(workspace) = resolve_path(workspace) else {
        return false;
    };
    let mut roots = TEMP_ROOTS.iter().map(PathBuf::from).collect::<Vec<_>>();
    roots.push(std::env::temp_dir());
    let roots = roots
        .iter()
        .filter_map(|root| resolve_path(root))
        .filter(|root| root.parent().is_some())
        .collect::<Vec<_>>();
    let Some(last_removal) = segments.iter().rposition(|segment| is_removal(segment)) else {
        return false;
    };
    // A failed `cd` followed by `;`/newline leaves the old cwd in effect.
    // Check both possible directories rather than assuming cd succeeded.
    let mut directories = vec![workspace.clone()];
    for (index, segment) in segments.iter().enumerate() {
        let tokens = tokenize(segment);
        let Some(head) = tokens.first() else {
            return false;
        };
        let args = tokens
            .iter()
            .skip(1)
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>();
        match head.text.as_str() {
            "rm" | "/bin/rm" | "/usr/bin/rm" | "rmdir" | "/bin/rmdir" | "/usr/bin/rmdir" => {
                if !removal_is_bounded(&head.text, &args, &directories, &workspace, &roots) {
                    return false;
                }
            }
            "cd" if index < last_removal => {
                if !track_directory(&args, &mut directories) {
                    return false;
                }
            }
            _ if index < last_removal => {
                if !OBSERVATION_HEADS.contains(&head.text.as_str())
                    || classify(segment) != CommandSafety::AlwaysSafe
                {
                    return false;
                }
            }
            _ => {
                if classify(segment) != CommandSafety::AlwaysSafe {
                    return false;
                }
            }
        }
    }
    // Pipes are allowed only after the last deletion, e.g. cleanup followed
    // by `git status --short | head`. A pipeline cannot establish our cwd.
    pipes_follow_removal(command, last_removal)
}

fn is_removal(segment: &str) -> bool {
    tokenize(segment).first().is_some_and(|token| {
        matches!(
            token.text.as_str(),
            "rm" | "/bin/rm" | "/usr/bin/rm" | "rmdir" | "/bin/rmdir" | "/usr/bin/rmdir"
        )
    })
}

fn has_dynamic_syntax(command: &str) -> bool {
    let mut quote = None;
    let mut chars = command.chars().peekable();
    while let Some(value) = chars.next() {
        if value == '\0' {
            return true;
        }
        if matches!(value, '\'' | '"') && quote.is_none_or(|active| active == value) {
            quote = if quote.is_some() { None } else { Some(value) };
        } else if quote == Some('\'') {
            continue;
        } else if matches!(value, '$' | '`' | '\\') {
            return true;
        } else if quote.is_none() {
            if matches!(
                value,
                '(' | ')' | '{' | '}' | '<' | '>' | '#' | '*' | '?' | '[' | ']' | '~' | '!'
            ) {
                return true;
            }
            if value == '&' && chars.next() != Some('&') {
                return true;
            }
            if value == '|' && chars.peek() == Some(&'|') {
                return true;
            }
        }
    }
    command.is_empty() || quote.is_some()
}

fn pipes_follow_removal(command: &str, last_removal: usize) -> bool {
    let mut quote = None;
    let mut segment = 0;
    let mut has_text = false;
    for value in command.chars() {
        if matches!(value, '\'' | '"') && quote.is_none_or(|active| active == value) {
            quote = if quote.is_some() { None } else { Some(value) };
        }
        if quote.is_none() && matches!(value, '|' | '&' | ';' | '\n' | '\r') {
            if value == '|' && segment < last_removal {
                return false;
            }
            if has_text {
                segment += 1;
                has_text = false;
            }
        } else if !value.is_whitespace() {
            has_text = true;
        }
    }
    true
}

fn track_directory(args: &[&str], directories: &mut Vec<PathBuf>) -> bool {
    let [target] = args else { return false };
    // Shell cd defaults to logical `..`, unlike filesystem path traversal.
    // Do not infer a cwd for that syntax through a symlink.
    if Path::new(target)
        .components()
        .any(|part| part == Component::ParentDir)
    {
        return false;
    }
    // Bare names can be redirected by CDPATH; -/~/variables are also dynamic.
    if !Path::new(target).is_absolute() && !target.starts_with("./") && !target.starts_with("../") {
        return false;
    }
    let next = directories
        .iter()
        .map(|cwd| resolve_path(&cwd.join(target)).filter(|path| path.is_dir()))
        .collect::<Option<Vec<_>>>();
    let Some(next) = next else { return false };
    for path in next {
        if !directories.contains(&path) {
            directories.push(path);
            if directories.len() > MAX_DIRECTORY_CANDIDATES {
                return false;
            }
        }
    }
    true
}

fn removal_is_bounded(
    head: &str,
    args: &[&str],
    directories: &[PathBuf],
    workspace: &Path,
    roots: &[PathBuf],
) -> bool {
    let mut options = true;
    let mut targets = 0;
    for arg in args {
        if options && *arg == "--" {
            options = false;
            continue;
        }
        if options && arg.starts_with('-') {
            let allowed = if head.ends_with("rmdir") {
                matches!(*arg, "-v" | "--verbose" | "--ignore-fail-on-non-empty")
            } else {
                RM_LONG_OPTIONS.contains(arg)
                    || (arg.starts_with('-')
                        && !arg.starts_with("--")
                        && arg.len() > 1
                        && arg[1..].chars().all(|value| "frRdviI".contains(value)))
            };
            if !allowed {
                return false;
            }
            continue;
        }
        targets += 1;
        if arg.is_empty()
            || !directories
                .iter()
                .all(|cwd| target_is_bounded(&cwd.join(arg), workspace, roots))
        {
            return false;
        }
    }
    targets > 0
}

fn target_is_bounded(target: &Path, workspace: &Path, roots: &[PathBuf]) -> bool {
    if target
        .components()
        .any(|component| component.as_os_str() == ".git")
    {
        return false;
    }
    let Some(target) = resolve_path(target) else {
        return false;
    };
    // Never remove the project, an ancestor of it, or a temporary root itself.
    if workspace.starts_with(&target)
        || roots.contains(&target)
        || target
            .components()
            .any(|component| component.as_os_str() == ".git")
    {
        return false;
    }
    target.starts_with(workspace) || roots.iter().any(|root| target.starts_with(root))
}

/// Resolve existing components (including symlinks) before handling `..`.
/// Missing final targets are normal for `rm -f`; permission/I/O errors fail closed.
fn resolve_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => resolved.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                match std::fs::symlink_metadata(&resolved) {
                    Ok(_) => resolved = std::fs::canonicalize(&resolved).ok()?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return None,
                }
            }
        }
    }
    Some(resolved)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("willdeep-removal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("build/nested")).unwrap();
        root
    }

    #[test]
    fn literal_cleanup_and_status_commands_are_bounded() {
        let root = fixture();
        let commands = [
            "rm -rf build",
            "rm -rf ./build/nested ../nonexistent-temp-artifact",
            "rm --force --recursive -- build 'artifact with spaces'",
            "rm -rf /tmp/willdeep-missing /private/tmp/willdeep-missing /var/tmp/willdeep-missing",
            "rmdir build/nested",
            "/bin/rm -rf build",
            "rm -rf build && rm -f artifact; git status --short | head -3; echo done",
            "rm -rf build; echo \"(空即干净)\"; git status --short | head -3",
            "rm -f 'literal*file' 'literal$file'",
        ];
        for command in commands {
            assert!(allows_full_access_removal(command, &root), "{command}");
            assert_eq!(
                classify(command),
                CommandSafety::AlwaysDangerous,
                "other modes unchanged: {command}"
            );
        }
        let absolute = format!(
            "rm -rf '{}'; cd '{}'; git status --short | head -3",
            root.join("build").display(),
            root.display()
        );
        assert!(allows_full_access_removal(&absolute, &root));
        assert!(allows_full_access_removal(
            "cd ./build && rm -rf nested",
            &root
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn roots_external_paths_and_unparsed_shell_still_require_approval() {
        let root = fixture();
        let commands = [
            "rm -rf /",
            "rm -rf .",
            "rm -rf build/..",
            "rm -rf /tmp",
            "rm -rf /private/tmp/.",
            "rm -rf /var/tmp",
            "rm -rf /etc/willdeep-test",
            "rm -rf /tmp/../etc/willdeep-test",
            "rm -rf /tmp-other/willdeep-test",
            "rm -rf build /etc/willdeep-test",
            "rm -rf .git",
            "rm -rf build/../.git/config",
            "rm -rf $TARGET",
            "rm -rf \"$TARGET\"",
            "rm -rf ${TMPDIR}/cache",
            "rm -rf $(pwd)/build",
            "rm -rf `pwd`/build",
            "rm -rf build/*",
            "rm -rf ~/cache",
            "sudo rm -rf build",
            "env rm -rf build",
            "sh -c 'rm -rf build'",
            "find build -delete",
            "rm -rf build; git reset --hard",
            "rm -rf build; rm -rf /etc/willdeep-test",
            "rm -rf build & rm -rf artifact",
            "ls | rm -rf build",
            "cd ./build | rm -rf nested",
            "cd /etc; rm -rf willdeep-test",
            "cd ./inside/..; rm -rf missing",
            "cd build && rm -rf nested",
            "rm -rf build || rm -rf artifact",
            "rm -rf build > /tmp/report",
            "rm -rf build\nln -s /etc outside\nrm -rf outside/foo",
            "rmdir -p build/nested",
            "rm --no-preserve-root -rf build",
            "rm -rf",
            "rm -rf 'build",
        ];
        for command in commands {
            assert!(!allows_full_access_removal(command, &root), "{command}");
        }
        for path in [&root, root.parent().unwrap()] {
            assert!(!allows_full_access_removal(
                &format!("rm -rf '{}'", path.display()),
                &root
            ));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_and_cd_failure_cannot_escape_the_allowed_paths() {
        let root = fixture();
        std::os::unix::fs::symlink("/etc", root.join("outside")).unwrap();
        std::os::unix::fs::symlink(root.join("build"), root.join("inside")).unwrap();
        for command in [
            "rm -rf outside",
            "rm -rf outside/hosts",
            "rm -rf outside/../etc/hosts",
        ] {
            assert!(!allows_full_access_removal(command, &root), "{command}");
        }
        assert!(allows_full_access_removal("rm -rf inside/nested", &root));
        // A failed second cd could leave /etc active. Checking only the last
        // successful-looking directory would incorrectly allow this command.
        let command = format!("cd /etc; cd '{}'; rm -rf missing", root.display());
        assert!(!allows_full_access_removal(&command, &root));
        std::fs::remove_dir_all(root).unwrap();
    }
}
