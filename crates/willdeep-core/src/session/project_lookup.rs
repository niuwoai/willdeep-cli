use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use uuid::Uuid;

use super::{SessionDigest, SessionStore, cached_digest, json_files, local_digest};

// SessionStore writes the workspace and ordering fields just before messages.
// Bound the read even when a legacy or externally written file has another layout.
const HEADER_LIMIT: u64 = 8 * 1024;
const MESSAGES_FIELD: &[u8] = b",\n  \"messages\": [";

type Rank = (u64, Uuid);

#[derive(Deserialize)]
struct Header {
    id: Uuid,
    workspace: PathBuf,
    updated_at: u64,
}

fn read_header(path: &Path) -> Option<Header> {
    let mut prefix = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(HEADER_LIMIT)
        .read_to_end(&mut prefix)
        .ok()?;
    let end = prefix
        .windows(MESSAGES_FIELD.len())
        .position(|window| window == MESSAGES_FIELD)?;
    prefix.truncate(end);
    prefix.push(b'}');
    serde_json::from_slice(&prefix).ok()
}

fn matches_workspace(candidate: &Path, workspace: &Path) -> bool {
    candidate.is_absolute()
        && (candidate == workspace || candidate.canonicalize().is_ok_and(|path| path == workspace))
}

fn rank(digest: &SessionDigest) -> Rank {
    (digest.updated_at, digest.id)
}

impl SessionStore {
    /// Latest native session with user input for this project. This lookup
    /// excludes bridged history and uses bounded header reads to rank candidates
    /// before decoding any transcript. Unrecognized layouts use the full parser.
    pub fn latest_project_digest(&self, workspace: &Path) -> Option<SessionDigest> {
        let workspace = workspace.canonicalize().ok()?;
        let mut ranked = Vec::new();
        let mut latest = None;
        let eligible = |path: &Path| {
            cached_digest(path, local_digest).filter(|digest| {
                digest.has_user_input && matches_workspace(&digest.workspace, &workspace)
            })
        };
        for path in json_files(&self.directory) {
            match read_header(&path) {
                Some(header) if matches_workspace(&header.workspace, &workspace) => {
                    ranked.push(((header.updated_at, header.id), path));
                }
                Some(_) => {}
                None => {
                    if let Some(digest) = eligible(&path)
                        && latest
                            .as_ref()
                            .is_none_or(|found| rank(&digest) > rank(found))
                    {
                        latest = Some(digest);
                    }
                }
            }
        }
        ranked.sort_unstable_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        for (header_rank, path) in ranked {
            if latest
                .as_ref()
                .is_some_and(|found| rank(found) > header_rank)
            {
                break;
            }
            if let Some(digest) = eligible(&path)
                && latest
                    .as_ref()
                    .is_none_or(|found| rank(&digest) > rank(found))
            {
                latest = Some(digest);
            }
        }
        latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, types::Message};

    #[test]
    fn project_lookup_skips_lower_ranked_transcripts_and_falls_back_for_legacy_layout() {
        let root = std::env::temp_dir().join(format!("willdeep-project-lookup-{}", Uuid::new_v4()));
        let workspace = root.join("project");
        let other = root.join("other");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let store = SessionStore::new(&root);
        std::fs::create_dir_all(&store.directory).unwrap();

        let write_session = |folder: &Path, updated_at, user_input, pretty| {
            let mut session = Session::new(folder.to_path_buf(), None, "");
            session.updated_at = updated_at;
            if user_input {
                session.messages.push(Message::user("previous question"));
            }
            let data = if pretty {
                serde_json::to_vec_pretty(&session)
            } else {
                serde_json::to_vec(&session)
            };
            std::fs::write(store.path(session.id), data.unwrap()).unwrap();
            session.id
        };

        let older = write_session(&workspace, 1, true, true);
        let selected = write_session(&workspace, 2, true, true);
        let unrelated = write_session(&other, 4, true, true);
        write_session(&workspace, 5, false, true);
        assert!(read_header(&store.path(selected)).is_some());
        assert_eq!(
            store.latest_project_digest(&workspace).unwrap().id,
            selected
        );
        {
            let cache = super::super::digest_cache().lock().unwrap();
            assert!(!cache.contains_key(&store.path(older)));
            assert!(!cache.contains_key(&store.path(unrelated)));
        }

        let compact = write_session(&workspace, 3, true, false);
        assert!(read_header(&store.path(compact)).is_none());
        assert_eq!(store.latest_project_digest(&workspace).unwrap().id, compact);
        std::fs::remove_dir_all(root).unwrap();
    }
}
