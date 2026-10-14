use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

const MAX_INSPECTED_ENTRIES: usize = 100_000;

/// Explicit task constraints; paths are workspace-relative glob patterns.
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CompletionContract {
    pub require_changed_globs: Vec<String>,
    pub preserve_existing_globs: Vec<String>,
}

impl CompletionContract {
    pub fn validate(&self) -> Result<(), String> {
        if self.require_changed_globs.len() + self.preserve_existing_globs.len() > 64 {
            return Err("completion contract supports at most 64 path patterns".into());
        }
        for pattern in self
            .require_changed_globs
            .iter()
            .chain(&self.preserve_existing_globs)
        {
            if pattern.is_empty()
                || Path::new(pattern).is_absolute()
                || pattern.split('/').any(|p| p == "..")
            {
                return Err("completion contract paths must stay inside the workspace".into());
            }
            Glob::new(pattern).map_err(|_| "invalid completion contract path pattern")?;
        }
        Ok(())
    }

    fn matches(&self) -> Result<GlobSet, String> {
        let mut builder = GlobSetBuilder::new();
        for pattern in self
            .require_changed_globs
            .iter()
            .chain(&self.preserve_existing_globs)
        {
            builder.add(Glob::new(pattern).map_err(|_| "invalid contract pattern")?);
        }
        builder
            .build()
            .map_err(|_| "cannot build contract patterns".into())
    }

    pub(super) fn capture(&self, workspace: &Path) -> Result<BTreeMap<String, [u8; 32]>, String> {
        let patterns = self.matches()?;
        let mut files = BTreeMap::new();
        let walk = ignore::WalkBuilder::new(workspace)
            .hidden(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false)
            .ignore(false)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build();
        for (index, entry) in walk.enumerate() {
            if index >= MAX_INSPECTED_ENTRIES {
                return Err(
                    "completion inspection limit exceeded; narrow the configured path scope".into(),
                );
            }
            let entry = entry.map_err(|_| "cannot inspect completion contract workspace")?;
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(workspace)
                .map_err(|_| "contract path escaped workspace")?;
            if !patterns.is_match(relative) {
                continue;
            }
            if entry.file_type().is_some_and(|t| t.is_symlink()) {
                return Err(
                    "completion contract matched a symlink; use an explicit regular-file scope"
                        .into(),
                );
            }
            let mut file = std::fs::File::open(entry.path())
                .map_err(|_| "cannot read completion contract file")?;
            let mut digest = Sha256::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let read = file
                    .read(&mut buffer)
                    .map_err(|_| "cannot read completion contract file")?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            files.insert(
                relative.to_string_lossy().replace('\\', "/"),
                digest.finalize().into(),
            );
        }
        Ok(files)
    }

    pub(super) fn feedback(
        &self,
        before: &BTreeMap<String, [u8; 32]>,
        after: &BTreeMap<String, [u8; 32]>,
    ) -> Option<String> {
        for pattern in &self.preserve_existing_globs {
            let matcher = Glob::new(pattern).ok()?.compile_matcher();
            if before
                .iter()
                .any(|(path, hash)| matcher.is_match(path) && after.get(path) != Some(hash))
            {
                return Some(format!(
                    "Existing files protected by {pattern:?} were changed or deleted. Restore the task's protected files; do not change tests to fit the fix."
                ));
            }
        }
        for pattern in &self.require_changed_globs {
            let matcher = Glob::new(pattern).ok()?.compile_matcher();
            let changed = before
                .keys()
                .chain(after.keys())
                .any(|path| matcher.is_match(path) && before.get(path) != after.get(path));
            if !changed {
                return Some(format!(
                    "No current-run implementation change satisfies {pattern:?}. Reproductions or changes in another worktree do not satisfy this contract. Integrate the actual fix and verify this workspace."
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contract_checks_current_run_changes_and_protects_existing_tests() {
        let contract = CompletionContract {
            require_changed_globs: vec!["src/**".into()],
            preserve_existing_globs: vec!["tests/**".into()],
        };
        contract.validate().unwrap();
        let before = BTreeMap::from([
            ("src/fix.rs".into(), [1; 32]),
            ("tests/old.rs".into(), [2; 32]),
        ]);
        assert!(
            contract
                .feedback(&before, &before)
                .unwrap()
                .contains("No current-run")
        );
        let mut after = before.clone();
        after.insert("src/fix.rs".into(), [3; 32]);
        assert!(contract.feedback(&before, &after).is_none());
        after.insert("tests/new.rs".into(), [4; 32]);
        assert!(contract.feedback(&before, &after).is_none());
        after.remove("tests/old.rs");
        assert!(
            contract
                .feedback(&before, &after)
                .unwrap()
                .contains("protected")
        );
    }
}
