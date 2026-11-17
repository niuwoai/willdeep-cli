//! Two-phase rename. Discovery identifies files only; none of its edits is used.
//! The final request sees every target opened at an exact, retained snapshot.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use super::client::{Client, Document, QueryContext, verify_snapshots};
use super::config::{Language, read_document, snapshot, uri};
use super::{Arguments, MAX_OPEN_DOCUMENTS, Operation, source_lines, validate_position};
use crate::tools::file_changes::{MAX_FILES, MAX_TOTAL_BYTES, VersionedReplacement};

pub(super) const MAX_NEW_NAME_BYTES: usize = 1024;
const MAX_TEXT_EDITS: usize = 10_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceEdit {
    // Intentionally no `changes`, resource operations or changeAnnotations.
    document_changes: Vec<DocumentEdit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentEdit {
    text_document: Identifier,
    edits: Vec<TextEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Identifier {
    uri: String,
    // Discovery can name an unopened document with null version. This is never
    // accepted as an edit: the final response must match our synchronized i32.
    version: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TextEdit {
    range: Range,
    new_text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Range {
    start: Position,
    end: Position,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct Position {
    line: u32,
    character: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeScriptWorkspaceEdit {
    changes: BTreeMap<String, Vec<TextEdit>>,
}

/// Only the language-aware parser can select the TypeScript native shape. A
/// missing documentChanges version remains server-versioned and fails conversion.
enum VersionSource {
    ServerVersioned(Option<i32>),
    TypeScriptSnapshotBound,
}

struct Target {
    path: PathBuf,
    uri: String,
    edits: Vec<TextEdit>,
    version_source: VersionSource,
}

struct Baseline {
    document: Document,
    text: String,
}

pub(super) fn validate_arguments(args: &Arguments) -> Result<(), String> {
    if args.operation != Operation::RenamePreview {
        if args.new_name.is_some() {
            return Err("new_name is only accepted by rename_preview".into());
        }
        return Ok(());
    }
    let name = args
        .new_name
        .as_deref()
        .ok_or("rename_preview requires new_name")?;
    if name.is_empty() || name.len() > MAX_NEW_NAME_BYTES || name.contains('\0') {
        return Err("new_name must contain 1 to 1024 UTF-8 bytes without NUL".into());
    }
    if args.limit.is_some() {
        return Err("rename_preview does not accept limit; edits cannot be truncated".into());
    }
    Ok(())
}

async fn request(
    client: &mut Client,
    args: &Arguments,
    document: &Document,
) -> Result<Value, String> {
    if !client.supports("renameProvider") {
        return Err("language server does not advertise renameProvider".into());
    }
    let revision = client.wait_until_ready().await?;
    let result = client
        .rpc
        .request(
            "textDocument/rename",
            json!({
                "textDocument":{"uri":document.uri},
                "position":{"line":args.line,"character":args.character},
                "newName":args.new_name,
            }),
        )
        .await?;
    client.verify_readiness(revision).await?;
    Ok(result)
}

/// Strict edit URI handling, separate from the query output filter. A bad
/// target rejects the complete edit rather than quietly filtering one file.
fn target_path(root: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.contains('\0') {
        return Err("rename target URI contains NUL".into());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "invalid rename target URI")?;
    if url.scheme() != "file"
        || url.host_str().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "rename target requires a local file URI without authority/query/fragment".into(),
        );
    }
    let lexical = url.to_file_path().map_err(|_| "invalid rename file URI")?;
    // Reject malformed/noncanonical percent escapes and lossy path decoding.
    if uri(&lexical)? != raw || lexical.as_os_str().as_encoded_bytes().contains(&0) {
        return Err("rename target URI is not a canonical file URI".into());
    }
    let real = lexical
        .canonicalize()
        .map_err(|e| format!("resolve rename target: {e}"))?;
    for path in [&lexical, &real] {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "rename target is outside workspace")?;
        if relative.components().any(|part| part.as_os_str() == ".git") {
            return Err("rename cannot target Git metadata".into());
        }
    }
    if !real.is_file() || Language::from_path(&real).is_err() {
        return Err("rename target must be a supported regular source file".into());
    }
    Ok(real)
}

fn targets(root: &Path, raw: Value) -> Result<Vec<Target>, String> {
    let edit: WorkspaceEdit = serde_json::from_value(raw).map_err(|e| {
        format!("rename requires versioned documentChanges without resources or annotations: {e}")
    })?;
    checked_targets(
        root,
        edit.document_changes.into_iter().map(|edit| {
            (
                edit.text_document.uri,
                edit.edits,
                VersionSource::ServerVersioned(edit.text_document.version),
            )
        }),
    )
}

fn targets_for_language(
    root: &Path,
    raw: Value,
    language: Language,
) -> Result<Vec<Target>, String> {
    if language == Language::TypeScript && raw.get("changes").is_some() {
        // typescript-language-server returns this native shape even when the
        // client advertises documentChanges. Do not manufacture LSP versions.
        // deny_unknown_fields rejects mixed shapes and all change annotations.
        let edit: TypeScriptWorkspaceEdit = serde_json::from_value(raw)
            .map_err(|e| format!("TypeScript rename requires only native changes without resources or annotations: {e}"))?;
        checked_targets(
            root,
            edit.changes
                .into_iter()
                .map(|(uri, edits)| (uri, edits, VersionSource::TypeScriptSnapshotBound)),
        )
    } else {
        targets(root, raw)
    }
}

fn checked_targets(
    root: &Path,
    edits: impl ExactSizeIterator<Item = (String, Vec<TextEdit>, VersionSource)>,
) -> Result<Vec<Target>, String> {
    if edits.len() == 0 || edits.len() > MAX_FILES {
        return Err("rename requires between 1 and 32 target files".into());
    }
    let mut seen = BTreeSet::new();
    let mut count = 0usize;
    let mut result = Vec::new();
    for (uri, edits, version_source) in edits {
        let path = target_path(root, &uri)?;
        if !seen.insert(path.clone()) {
            return Err("duplicate canonical rename destination".into());
        }
        count = count
            .checked_add(edits.len())
            .ok_or("rename edit count overflow")?;
        if edits.is_empty() || count > MAX_TEXT_EDITS {
            return Err("rename requires nonempty edits and at most 10000 text edits".into());
        }
        for text_edit in &edits {
            if text_edit.new_text.contains('\0') {
                return Err("rename replacement contains NUL".into());
            }
            if text_edit.range.start > text_edit.range.end {
                return Err("rename range end precedes start".into());
            }
        }
        result.push(Target {
            path,
            uri,
            edits,
            version_source,
        });
    }
    Ok(result)
}

pub(super) async fn replacements(
    client: &mut Client,
    root: &Path,
    args: &Arguments,
    context: &QueryContext,
) -> Result<Vec<VersionedReplacement>, String> {
    validate_arguments(args)?;
    validate_position(&context.text, args.line, args.character)?;
    if context.text.contains('\0') {
        return Err("rename source snapshot contains NUL".into());
    }
    let language = client.language();
    let discovery = targets_for_language(
        root,
        request(client, args, &context.document).await?,
        language,
    )?;
    verify_snapshots(root, &context.checked_documents).await?;
    let mut required: BTreeSet<_> = discovery.into_iter().map(|target| target.path).collect();
    required.insert(context.document.path.clone());
    if required.len() > MAX_OPEN_DOCUMENTS {
        return Err("rename source and discovered targets exceed 32 open documents".into());
    }
    // Reserve all target overlays together. synchronize's ordinary LRU-style
    // eviction must never evict a target opened earlier in this same phase.
    let retire: Vec<_> = client
        .documents
        .values()
        .filter(|document| !required.contains(&document.path))
        .map(|document| document.uri.clone())
        .collect();
    for raw_uri in retire {
        client.close(&raw_uri).await?;
    }
    let prior: BTreeMap<_, _> = context
        .checked_documents
        .iter()
        .map(|document| (document.path.clone(), document))
        .collect();
    let mut baselines = BTreeMap::new();
    let mut bytes = 0usize;
    for path in required {
        // Resolve again before reading a discovered target; aliases may have
        // been retargeted while discovery was in flight.
        if target_path(root, &uri(&path)?)? != path {
            return Err("rename target real path changed before synchronization".into());
        }
        let text = read_document(&path).await?;
        if text.contains('\0') {
            return Err("rename target snapshot contains NUL".into());
        }
        if prior
            .get(&path)
            .is_some_and(|document| document.snapshot != snapshot(text.as_bytes()))
        {
            return Err("opened rename source changed during discovery; retry".into());
        }
        bytes = bytes
            .checked_add(text.len())
            .ok_or("rename snapshot size overflow")?;
        if bytes > MAX_TOTAL_BYTES {
            return Err("rename snapshots exceed the 4 MiB preview budget".into());
        }
        let document = client.synchronize(&path, &text).await?;
        baselines.insert(path, Baseline { document, text });
    }
    let checked: Vec<_> = baselines
        .values()
        .map(|base| base.document.clone())
        .collect();
    verify_snapshots(root, &context.checked_documents).await?;
    verify_snapshots(root, &checked).await?;
    let source = &baselines
        .get(&context.document.path)
        .ok_or("rename source snapshot missing")?
        .document;
    let final_edit = request(client, args, source).await?;
    // These reads verify hashes only. All ranges below use retained pre-request
    // text, never any bytes first read after the final response.
    verify_snapshots(root, &context.checked_documents).await?;
    verify_snapshots(root, &checked).await?;
    convert(
        root,
        targets_for_language(root, final_edit, language)?,
        &baselines,
    )
}

fn convert(
    root: &Path,
    targets: Vec<Target>,
    baselines: &BTreeMap<PathBuf, Baseline>,
) -> Result<Vec<VersionedReplacement>, String> {
    let mut replacements = Vec::with_capacity(targets.len());
    let mut total = 0usize;
    for target in targets {
        let baseline = baselines.get(&target.path).ok_or(
            "final rename target has no pre-request synchronized snapshot; retry discovery",
        )?;
        // Versions belong to a synchronized URI, not to a canonical inode.
        // An unopened symlink alias can coincidentally carry the same integer.
        if target.uri != baseline.document.uri {
            return Err("final rename document URI was not synchronized; retry discovery".into());
        }
        match target.version_source {
            VersionSource::ServerVersioned(version) => {
                let version = version.ok_or("final rename document version is missing or null")?;
                if i64::from(version) != baseline.document.version {
                    return Err("final rename document version is unknown or stale".into());
                }
            }
            // Native TypeScript changes are bound to the exact URI, text and
            // SHA256 retained before the final request, verified on both sides
            // of that request. They never supply or inherit a server version.
            VersionSource::TypeScriptSnapshotBound => {}
        }
        let content = apply_edits(&baseline.text, &target.edits)?;
        total = total
            .checked_add(baseline.text.len())
            .and_then(|size| size.checked_add(content.len()))
            .ok_or("rename preview size overflow")?;
        if total > MAX_TOTAL_BYTES {
            return Err("combined old/new rename content exceeds the 4 MiB preview limit".into());
        }
        let path = target
            .path
            .strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .ok_or("rename target requires a UTF-8 workspace-relative path")?
            .to_owned();
        replacements.push(VersionedReplacement {
            path,
            expected_version: baseline.document.snapshot.clone(),
            content,
        });
    }
    Ok(replacements)
}

/// Resolve all requested positions with one scan, so many edits on a long line
/// cannot turn UTF-16 validation into O(file bytes * edit count).
fn offsets(text: &str, edits: &[TextEdit]) -> Result<BTreeMap<Position, usize>, String> {
    let wanted: BTreeSet<_> = edits
        .iter()
        .flat_map(|edit| [edit.range.start, edit.range.end])
        .collect();
    let mut pending = wanted.iter().peekable();
    let mut result = BTreeMap::new();
    for (line, (base, row)) in source_lines(text).enumerate() {
        let mut units = 0u32;
        for (column, ch) in row.char_indices().chain(std::iter::once((row.len(), '\0'))) {
            let position = Position {
                line: line as u32,
                character: units,
            };
            if pending.peek().is_some_and(|wanted| **wanted < position) {
                return Err(
                    "rename position is out of range or splits a UTF-16 surrogate pair".into(),
                );
            }
            if pending.peek().is_some_and(|wanted| **wanted == position) {
                result.insert(position, base + column);
                pending.next();
            }
            units += ch.len_utf16() as u32;
        }
        if pending
            .peek()
            .is_some_and(|wanted| wanted.line == line as u32)
        {
            return Err("rename character is outside source line".into());
        }
    }
    if pending.next().is_some() {
        return Err("rename line is outside source snapshot".into());
    }
    Ok(result)
}

fn apply_edits(text: &str, edits: &[TextEdit]) -> Result<String, String> {
    if text.contains('\0') {
        return Err("rename snapshot contains NUL".into());
    }
    let positions = offsets(text, edits)?;
    let mut spans = Vec::with_capacity(edits.len());
    let mut size = text.len();
    for edit in edits {
        if edit.range.start > edit.range.end || edit.new_text.contains('\0') {
            return Err("rename range is reversed or replacement contains NUL".into());
        }
        let start = positions[&edit.range.start];
        let end = positions[&edit.range.end];
        size = size
            .checked_sub(end - start)
            .and_then(|size| size.checked_add(edit.new_text.len()))
            .ok_or("rename output size overflow")?;
        if size > MAX_TOTAL_BYTES {
            return Err("rename replacement exceeds the 4 MiB preview budget".into());
        }
        spans.push((start, end, edit.new_text.as_str()));
    }
    spans.sort_unstable_by_key(|(start, end, _)| (*start, *end));
    for pair in spans.windows(2) {
        if pair[0].1 > pair[1].0 || pair[0].0 == pair[1].0 {
            return Err("overlapping or ambiguous rename edits".into());
        }
    }
    let mut result = String::with_capacity(size);
    let mut cursor = 0;
    for (start, end, replacement) in spans {
        result.push_str(&text[cursor..start]);
        result.push_str(replacement);
        cursor = end;
    }
    result.push_str(&text[cursor..]);
    Ok(result)
}

#[cfg(test)]
#[path = "rename_tests.rs"]
mod tests;
