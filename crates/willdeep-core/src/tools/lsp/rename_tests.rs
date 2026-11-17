use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;
use crate::tools::lsp::tests::{FakeServer, Fixture, fake_language_client};
use crate::tools::{ApprovalDecision, ApprovalMode, Approver, ToolCall, ToolRegistry};

fn arguments() -> Arguments {
    Arguments {
        operation: Operation::RenamePreview,
        path: Some("a.ts".into()),
        line: Some(0),
        character: Some(0),
        query: None,
        new_name: Some("newKey".into()),
        limit: None,
        timeout_seconds: None,
    }
}

fn position(text: &str, byte: usize) -> Value {
    let prefix = &text[..byte];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let character = prefix.rsplit('\n').next().unwrap().encode_utf16().count();
    json!({"line":line,"character":character})
}

fn edit(text: &str, start: usize, end: usize, replacement: &str) -> Value {
    json!({"range":{"start":position(text, start),"end":position(text, end)},"newText":replacement})
}

fn edit_at(start: (u32, u32), end: (u32, u32), replacement: &str) -> Value {
    json!({"range":{"start":{"line":start.0,"character":start.1},
        "end":{"line":end.0,"character":end.1}},"newText":replacement})
}

fn document(raw_uri: &str, version: Value, edits: Vec<Value>) -> Value {
    json!({"textDocument":{"uri":raw_uri,"version":version},"edits":edits})
}

fn workspace(documents: Vec<Value>) -> Value {
    json!({"documentChanges":documents})
}

fn workspace_for_shape(documents: Vec<Value>, native_changes: bool) -> Value {
    if !native_changes {
        return workspace(documents);
    }
    let changes: BTreeMap<_, _> = documents
        .into_iter()
        .map(|document| {
            (
                document["textDocument"]["uri"].as_str().unwrap().to_owned(),
                document["edits"].clone(),
            )
        })
        .collect();
    json!({"changes":changes})
}

async fn prepared(fixture: &Fixture) -> (Client, QueryContext, FakeServer) {
    prepared_language(fixture, Language::TypeScript).await
}

async fn prepared_language(
    fixture: &Fixture,
    language: Language,
) -> (Client, QueryContext, FakeServer) {
    let (mut client, mut server) = fake_language_client(
        &fixture.root,
        language,
        json!({
            "textDocumentSync":{"openClose":true,"change":1},
            "renameProvider":{"prepareProvider":false},"positionEncoding":"utf-16"
        }),
    )
    .await;
    let path = fixture.root.join(match language {
        Language::Rust => "a.rs",
        Language::TypeScript => "a.ts",
    });
    let context = client
        .refresh_documents(&fixture.root, &path)
        .await
        .unwrap();
    let opened = server.next().await;
    assert_eq!(opened["method"], "textDocument/didOpen");
    assert_eq!(
        opened["params"]["textDocument"]["languageId"],
        language.document_language(&path)
    );
    (client, context, server)
}

async fn reply(server: &mut FakeServer, request: &Value, result: Value) {
    server
        .send(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
        .await;
}

/// Return the final request and exact versions observed in preceding open/change
/// notifications, proving the real client synchronized every final target.
async fn final_request(server: &mut FakeServer) -> (Value, BTreeMap<String, Value>) {
    let (request, overlays) = final_request_with_overlays(server).await;
    let versions = overlays
        .into_iter()
        .map(|(uri, message)| (uri, message["params"]["textDocument"]["version"].clone()))
        .collect();
    (request, versions)
}

async fn final_request_with_overlays(server: &mut FakeServer) -> (Value, BTreeMap<String, Value>) {
    let mut overlays = BTreeMap::new();
    loop {
        let message = server.next().await;
        if message["method"] == "textDocument/rename" {
            return (message, overlays);
        }
        assert!(matches!(
            message["method"].as_str(),
            Some("textDocument/didOpen" | "textDocument/didChange" | "textDocument/didClose")
        ));
        if let Some(raw_uri) = message["params"]["textDocument"]["uri"].as_str()
            && message["method"] != "textDocument/didClose"
        {
            overlays.insert(raw_uri.to_owned(), message);
        }
    }
}

struct CountApproval(AtomicUsize);

#[async_trait::async_trait]
impl Approver for CountApproval {
    async fn approve(&self, description: &str, always_allow_available: bool) -> ApprovalDecision {
        assert!(!always_allow_available);
        assert!(description.contains("binding"));
        self.0.fetch_add(1, Ordering::SeqCst);
        ApprovalDecision::AllowOnce
    }
}

#[tokio::test]
async fn concrete_client_rename_opens_cross_file_targets_then_applies_one_bound_preview() {
    cross_file_preview(false).await;
}

#[tokio::test]
async fn concrete_typescript_client_native_changes_applies_cross_file_snapshot_bound_preview() {
    cross_file_preview(true).await;
}

async fn cross_file_preview(native_changes: bool) {
    let fixture = Fixture::new();
    let a = "export const item = { oldKey: 1 };\nconst unrelated = 'oldKey';\n";
    let b = "import { item } from './a';\nconst emoji = '😀'; item['oldKey']; item.oldKey;\nconst label = 'oldKey';\n";
    fixture.write("a.ts", a);
    fixture.write("b.ts", b);
    let (mut client, context, mut server) = prepared(&fixture).await;
    assert_eq!(client.capability_summary()["rename_preview"], true);
    let a_uri = uri(&fixture.root.join("a.ts")).unwrap();
    let b_uri = uri(&fixture.root.join("b.ts")).unwrap();
    let a_start = a.find("oldKey").unwrap();
    let b_start = b.find("oldKey").unwrap();
    let b_second = b[b_start + 6..].find("oldKey").unwrap() + b_start + 6;
    let mut args = arguments();
    args.character = Some(a[..a_start].encode_utf16().count() as u32);
    let a_edits = vec![edit(a, a_start, a_start + 6, "newKey")];
    // Deliberately unsorted, including a quoted property after an astral scalar.
    let b_edits = vec![
        edit(b, b_second, b_second + 6, "newKey"),
        edit(b, b_start, b_start + 6, "newKey"),
    ];
    let fake = tokio::spawn(async move {
        let discovery = server.next().await;
        assert_eq!(discovery["method"], "textDocument/rename");
        assert_eq!(discovery["params"]["newName"], "newKey");
        assert_eq!(
            discovery["params"]["position"],
            json!({"line":0,"character":a_start})
        );
        let discovery_edits = vec![edit_at((999, 0), (999, 1), "discovery-only")];
        reply(
            &mut server,
            &discovery,
            workspace_for_shape(
                vec![
                    document(&a_uri, json!(1), discovery_edits.clone()),
                    document(&b_uri, Value::Null, discovery_edits),
                ],
                native_changes,
            ),
        )
        .await;
        let (final_query, overlays) = final_request_with_overlays(&mut server).await;
        assert_eq!(overlays.len(), 2);
        assert_eq!(overlays[&a_uri]["method"], "textDocument/didChange");
        assert_eq!(
            overlays[&a_uri]["params"]["contentChanges"],
            json!([{"text":a}])
        );
        assert_eq!(overlays[&b_uri]["method"], "textDocument/didOpen");
        assert_eq!(overlays[&b_uri]["params"]["textDocument"]["text"], b);
        assert_eq!(final_query["params"]["textDocument"]["uri"], a_uri);
        assert_eq!(
            final_query["params"]["position"],
            discovery["params"]["position"]
        );
        let a_version = overlays[&a_uri]["params"]["textDocument"]["version"].clone();
        let b_version = overlays[&b_uri]["params"]["textDocument"]["version"].clone();
        assert!(a_version.as_i64().unwrap() > 1);
        assert!(b_version.as_i64().unwrap() > 1);
        reply(
            &mut server,
            &final_query,
            workspace_for_shape(
                vec![
                    document(&a_uri, a_version, a_edits),
                    document(&b_uri, b_version, b_edits),
                ],
                native_changes,
            ),
        )
        .await;
        server
    });
    let changes = replacements(&mut client, &fixture.root, &args, &context)
        .await
        .unwrap();
    let _server = fake.await.unwrap();
    assert_eq!(changes.len(), 2);
    for change in &changes {
        let original = match change.path.as_str() {
            "a.ts" => a,
            "b.ts" => b,
            other => panic!("unexpected rename target: {other}"),
        };
        assert_eq!(change.expected_version, snapshot(original.as_bytes()));
    }
    let approver = Arc::new(CountApproval(AtomicUsize::new(0)));
    let registry = ToolRegistry::new(&fixture.root, ApprovalMode::Strict)
        .unwrap()
        .with_approver(approver.clone());
    let preview: Value = serde_json::from_str(
        &registry
            .preview_versioned_replacements(changes)
            .await
            .unwrap()
            .publish(),
    )
    .unwrap();
    assert_eq!(preview["status"], "preview");
    assert_eq!(preview["file_count"], 2);
    assert_eq!(preview["files_touched"], json!([]));
    assert_eq!(approver.0.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("a.ts")).unwrap(),
        a
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("b.ts")).unwrap(),
        b
    );
    let call = ToolCall {
        id: "apply-rename".into(),
        name: "file_changes".into(),
        arguments: json!({"operation":"apply","preview_id":preview["preview_id"]}).to_string(),
    };
    let receipt: Value =
        serde_json::from_str(&registry.file_changes(&call).await.unwrap()).unwrap();
    assert_eq!(receipt["status"], "applied");
    assert_eq!(receipt["files_touched"], json!(["a.ts", "b.ts"]));
    assert_eq!(approver.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("a.ts")).unwrap(),
        "export const item = { newKey: 1 };\nconst unrelated = 'oldKey';\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("b.ts")).unwrap(),
        "import { item } from './a';\nconst emoji = '😀'; item['newKey']; item.newKey;\nconst label = 'oldKey';\n"
    );
    assert!(
        registry.file_changes(&call).await.is_err(),
        "preview is one-shot"
    );
}

#[test]
fn utf16_ranges_handle_crlf_eof_and_multiline_replacements() {
    let text = "😀old\r\nnext\r\n";
    let edits: Vec<TextEdit> = serde_json::from_value(json!([
        edit_at((1, 0), (2, 0), "end\r\n"),
        edit_at((0, 2), (0, 5), "new"),
    ]))
    .unwrap();
    assert_eq!(apply_edits(text, &edits).unwrap(), "😀new\r\nend\r\n");
    let edits: Vec<TextEdit> =
        serde_json::from_value(json!([edit_at((0, 2), (1, 2), "span"),])).unwrap();
    assert_eq!(apply_edits(text, &edits).unwrap(), "😀spanxt\r\n");
    let text = "😀old\rnext\r";
    let edits: Vec<TextEdit> = serde_json::from_value(json!([
        edit_at((1, 0), (2, 0), "end\r"),
        edit_at((0, 2), (0, 5), "new"),
    ]))
    .unwrap();
    assert_eq!(apply_edits(text, &edits).unwrap(), "😀new\rend\r");
    assert!(validate_position(text, Some(1), Some(4)).is_ok());
    assert!(validate_position(text, Some(0), Some(6)).is_err());
}

#[test]
fn invalid_utf16_out_of_bounds_reversed_overlapping_and_nul_edits_fail_whole_conversion() {
    let text = "😀old\r\nnext";
    let cases = vec![
        vec![edit_at((0, 1), (0, 2), "x")],
        vec![edit_at((0, 0), (0, 1), "x")],
        vec![edit_at((0, 6), (0, 6), "x")],
        vec![edit_at((2, 0), (2, 0), "x")],
        vec![edit_at((1, 1), (0, 2), "x")],
        vec![edit_at((0, 2), (0, 5), "x"), edit_at((0, 3), (0, 4), "y")],
        vec![edit_at((0, 2), (0, 2), "x"), edit_at((0, 2), (0, 2), "y")],
        vec![edit_at((0, 2), (0, 5), "x"), edit_at((0, 2), (0, 2), "y")],
        vec![edit_at((0, 2), (0, 5), "bad\0text")],
    ];
    for case in cases {
        let edits: Vec<TextEdit> = serde_json::from_value(json!(case)).unwrap();
        assert!(apply_edits(text, &edits).is_err());
    }
    let edits: Vec<TextEdit> =
        serde_json::from_value(json!([edit_at((0, 0), (0, 0), "x")])).unwrap();
    assert!(apply_edits("bad\0source", &edits).is_err());
}

#[test]
fn workspace_edits_reject_resources_annotations_unversioned_shapes_and_malformed_positions() {
    let fixture = Fixture::new();
    let path = fixture.write("a.ts", "old");
    let raw_uri = uri(&path).unwrap();
    let good = document(&raw_uri, json!(1), vec![edit_at((0, 0), (0, 3), "new")]);
    let mut annotated = good.clone();
    annotated["edits"][0]["annotationId"] = json!("confirm");
    let mut negative = good.clone();
    negative["edits"][0]["range"]["start"]["character"] = json!(-1);
    let mut large = good.clone();
    large["edits"][0]["range"]["end"]["line"] = json!(u64::MAX);
    let mut missing = good.clone();
    missing["edits"][0]["range"]["start"]
        .as_object_mut()
        .unwrap()
        .remove("line");
    for invalid in [
        Value::Null,
        json!({"changes":{(raw_uri.clone()):[]}}),
        json!({"documentChanges":[good.clone()],"changeAnnotations":{}}),
        workspace(vec![good.clone(), json!({"kind":"create","uri":raw_uri})]),
        workspace(vec![
            good.clone(),
            json!({"kind":"rename","oldUri":raw_uri,"newUri":raw_uri}),
        ]),
        workspace(vec![good.clone(), json!({"kind":"delete","uri":raw_uri})]),
        workspace(vec![annotated]),
        workspace(vec![negative]),
        workspace(vec![large]),
        workspace(vec![missing]),
        workspace(vec![document(&raw_uri, json!(1), vec![])]),
        workspace(vec![document(
            &raw_uri,
            json!(1),
            vec![edit_at((0, 0), (0, 0), "\0")],
        )]),
        workspace(vec![good.clone(), good.clone()]),
    ] {
        assert!(targets(&fixture.root, invalid).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }
    assert!(targets(&fixture.root, workspace(vec![good])).is_ok());
}

#[test]
fn rename_uris_reject_external_targets_authorities_queries_fragments_and_bad_escapes() {
    let fixture = Fixture::new();
    let path = fixture.write("a.ts", "old");
    let external = Fixture::new();
    let outside = external.write("outside.ts", "outside");
    let raw_uri = uri(&path).unwrap();
    for invalid in [
        "not a URI".to_owned(),
        "https://example.com/a.ts".to_owned(),
        "file://remote.invalid/a.ts".to_owned(),
        uri(&outside).unwrap(),
        format!("{raw_uri}?query"),
        format!("{raw_uri}#fragment"),
        raw_uri.replace("a.ts", "%61.ts"),
        raw_uri.replace("a.ts", "%00.ts"),
        raw_uri.replace("a.ts", "%zz.ts"),
        format!("{raw_uri}\0"),
    ] {
        assert!(target_path(&fixture.root, &invalid).is_err(), "{invalid:?}");
    }
    let git = fixture.write(".git/a.ts", "metadata");
    assert!(target_path(&fixture.root, &uri(&git).unwrap()).is_err());
}

#[cfg(unix)]
#[test]
fn rename_uris_reject_duplicate_canonical_aliases_and_symlinks_leaving_workspace() {
    let fixture = Fixture::new();
    let path = fixture.write("a.ts", "old");
    let alias = fixture.root.join("alias.ts");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let e = vec![edit_at((0, 0), (0, 3), "new")];
    assert!(
        targets(
            &fixture.root,
            workspace(vec![
                document(&uri(&path).unwrap(), json!(1), e.clone()),
                document(&uri(&alias).unwrap(), json!(1), e),
            ])
        )
        .err()
        .unwrap()
        .contains("duplicate canonical")
    );
    let outside = Fixture::new();
    let external = outside.write("outside.ts", "outside");
    let link = fixture.root.join("external.ts");
    std::os::unix::fs::symlink(external, &link).unwrap();
    assert!(target_path(&fixture.root, &uri(&link).unwrap()).is_err());
}

fn baseline(fixture: &Fixture, path: &str, text: &str) -> Baseline {
    let context = fixture.context(path, text);
    Baseline {
        document: context.document,
        text: text.into(),
    }
}

#[test]
fn final_edit_versions_must_match_pre_request_snapshots_and_cannot_add_unknown_targets() {
    let fixture = Fixture::new();
    let base = baseline(&fixture, "a.ts", "old");
    let raw_uri = base.document.uri.clone();
    let baselines = BTreeMap::from([(base.document.path.clone(), base)]);
    let edits = vec![edit_at((0, 0), (0, 3), "new")];
    for version in [Value::Null, json!(0), json!(6), json!(8), json!(-1)] {
        let target = targets(
            &fixture.root,
            workspace(vec![document(&raw_uri, version, edits.clone())]),
        )
        .unwrap();
        assert!(convert(&fixture.root, target, &baselines).is_err());
    }
    let mut omitted = document(&raw_uri, json!(7), edits.clone());
    omitted["textDocument"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    assert!(
        convert(
            &fixture.root,
            targets(&fixture.root, workspace(vec![omitted])).unwrap(),
            &baselines
        )
        .is_err()
    );
    let b = fixture.write("b.ts", "old");
    let unknown = targets(
        &fixture.root,
        workspace(vec![
            document(&raw_uri, json!(7), edits.clone()),
            document(&uri(&b).unwrap(), json!(7), edits.clone()),
        ]),
    )
    .unwrap();
    assert!(
        convert(&fixture.root, unknown, &baselines)
            .err()
            .unwrap()
            .contains("no pre-request")
    );
    let result = convert(
        &fixture.root,
        targets(
            &fixture.root,
            workspace(vec![document(&raw_uri, json!(7), edits)]),
        )
        .unwrap(),
        &baselines,
    )
    .unwrap();
    assert_eq!(result[0].expected_version, snapshot(b"old"));
    assert_eq!(result[0].content, "new");
}

#[tokio::test]
async fn concrete_client_rejects_targets_first_introduced_by_final_response() {
    for native_changes in [false, true] {
        let fixture = Fixture::new();
        fixture.write("a.ts", "old");
        let b = fixture.write("b.ts", "old");
        let b_uri = uri(&b).unwrap();
        let (mut client, context, mut server) = prepared(&fixture).await;
        let a_uri = context.document.uri.clone();
        let fake = tokio::spawn(async move {
            let edits = vec![edit_at((0, 0), (0, 3), "new")];
            let discovery = server.next().await;
            reply(
                &mut server,
                &discovery,
                workspace_for_shape(
                    vec![document(&a_uri, json!(1), edits.clone())],
                    native_changes,
                ),
            )
            .await;
            let (final_query, versions) = final_request(&mut server).await;
            assert!(!versions.contains_key(&b_uri));
            reply(
                &mut server,
                &final_query,
                workspace_for_shape(
                    vec![
                        document(&a_uri, versions[&a_uri].clone(), edits.clone()),
                        document(&b_uri, json!(99), edits),
                    ],
                    native_changes,
                ),
            )
            .await;
            server
        });
        let error = replacements(&mut client, &fixture.root, &arguments(), &context)
            .await
            .err()
            .unwrap();
        assert!(error.contains("no pre-request"), "{error}");
        let _server = fake.await.unwrap();
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("a.ts")).unwrap(),
            "old"
        );
        assert_eq!(std::fs::read_to_string(b).unwrap(), "old");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn concrete_client_rejects_single_alias_with_matching_synchronized_version() {
    for native_changes in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.write("a.ts", "old");
        let alias = fixture.root.join("alias.ts");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let alias_uri = uri(&alias).unwrap();
        let (mut client, context, mut server) = prepared(&fixture).await;
        let a_uri = context.document.uri.clone();
        let fake = tokio::spawn(async move {
            let edits = vec![edit_at((0, 0), (0, 3), "new")];
            let discovery = server.next().await;
            // Discovery may identify a canonical destination through an alias.
            reply(
                &mut server,
                &discovery,
                workspace_for_shape(
                    vec![document(&alias_uri, Value::Null, edits.clone())],
                    native_changes,
                ),
            )
            .await;
            let (final_query, versions) = final_request(&mut server).await;
            assert!(versions.contains_key(&a_uri));
            assert!(
                !versions.contains_key(&alias_uri),
                "alias overlay was never synchronized"
            );
            // The integer is valid for a.ts, but says nothing about alias.ts.
            reply(
                &mut server,
                &final_query,
                workspace_for_shape(
                    vec![document(&alias_uri, versions[&a_uri].clone(), edits)],
                    native_changes,
                ),
            )
            .await;
            server
        });
        let error = replacements(&mut client, &fixture.root, &arguments(), &context)
            .await
            .err()
            .unwrap();
        assert!(error.contains("URI was not synchronized"), "{error}");
        let _server = fake.await.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(alias).unwrap(), "old");
    }
}

#[tokio::test]
async fn concrete_client_rejects_changes_during_discovery_or_final_rename() {
    for native_changes in [false, true] {
        for (during_discovery, changed_file) in [(true, "a.ts"), (false, "a.ts"), (false, "b.ts")] {
            let fixture = Fixture::new();
            fixture.write("a.ts", "old");
            let b = fixture.write("b.ts", "old");
            let b_uri = uri(&b).unwrap();
            let changed_path = fixture.root.join(changed_file);
            let (mut client, context, mut server) = prepared(&fixture).await;
            let a_uri = context.document.uri.clone();
            let fake = tokio::spawn(async move {
                let discovery = server.next().await;
                let edits = vec![edit_at((0, 0), (0, 3), "new")];
                if during_discovery {
                    std::fs::write(&changed_path, "external").unwrap();
                }
                reply(
                    &mut server,
                    &discovery,
                    workspace_for_shape(
                        vec![
                            document(&a_uri, json!(1), edits.clone()),
                            document(&b_uri, Value::Null, edits.clone()),
                        ],
                        native_changes,
                    ),
                )
                .await;
                if !during_discovery {
                    let (final_query, versions) = final_request(&mut server).await;
                    std::fs::write(&changed_path, "external").unwrap();
                    reply(
                        &mut server,
                        &final_query,
                        workspace_for_shape(
                            vec![
                                document(&a_uri, versions[&a_uri].clone(), edits.clone()),
                                document(&b_uri, versions[&b_uri].clone(), edits),
                            ],
                            native_changes,
                        ),
                    )
                    .await;
                }
                server
            });
            let error = replacements(&mut client, &fixture.root, &arguments(), &context)
                .await
                .err()
                .unwrap();
            assert!(error.contains("stale result discarded"), "{error}");
            let _server = fake.await.unwrap();
            assert_eq!(
                std::fs::read_to_string(fixture.root.join(changed_file)).unwrap(),
                "external"
            );
            let unchanged = if changed_file == "a.ts" {
                "b.ts"
            } else {
                "a.ts"
            };
            assert_eq!(
                std::fs::read_to_string(fixture.root.join(unchanged)).unwrap(),
                "old"
            );
        }
    }
}

#[tokio::test]
async fn concrete_rust_client_rejects_native_changes_in_either_phase() {
    for final_phase in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.write("a.rs", "old");
        let (mut client, context, mut server) = prepared_language(&fixture, Language::Rust).await;
        let raw_uri = context.document.uri.clone();
        let fake = tokio::spawn(async move {
            let edits = vec![edit_at((0, 0), (0, 3), "new")];
            let native = json!({"changes":{(raw_uri.clone()):edits.clone()}});
            let discovery = server.next().await;
            if final_phase {
                reply(
                    &mut server,
                    &discovery,
                    workspace(vec![document(&raw_uri, json!(1), edits)]),
                )
                .await;
                let (final_query, _) = final_request(&mut server).await;
                reply(&mut server, &final_query, native).await;
            } else {
                reply(&mut server, &discovery, native).await;
            }
            server
        });
        let mut args = arguments();
        args.path = Some("a.rs".into());
        let error = replacements(&mut client, &fixture.root, &args, &context)
            .await
            .err()
            .unwrap();
        assert!(
            error.contains("requires versioned documentChanges"),
            "{error}"
        );
        let _server = fake.await.unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "old");
    }
}

#[tokio::test]
async fn concrete_clients_never_treat_missing_null_or_stale_document_versions_as_snapshot_bound() {
    for language in [Language::Rust, Language::TypeScript] {
        for version in [None, Some(Value::Null), Some(json!(1))] {
            let fixture = Fixture::new();
            let name = match language {
                Language::Rust => "a.rs",
                Language::TypeScript => "a.ts",
            };
            let path = fixture.write(name, "old");
            let (mut client, context, mut server) = prepared_language(&fixture, language).await;
            let raw_uri = context.document.uri.clone();
            let missing_or_null = version.as_ref().is_none_or(Value::is_null);
            let fake = tokio::spawn(async move {
                let edits = vec![edit_at((0, 0), (0, 3), "new")];
                let discovery = server.next().await;
                reply(
                    &mut server,
                    &discovery,
                    workspace(vec![document(&raw_uri, Value::Null, edits.clone())]),
                )
                .await;
                let (final_query, versions) = final_request(&mut server).await;
                assert!(versions[&raw_uri].as_i64().unwrap() > 1, "1 is now stale");
                let mut final_document =
                    document(&raw_uri, version.clone().unwrap_or(Value::Null), edits);
                if version.is_none() {
                    final_document["textDocument"]
                        .as_object_mut()
                        .unwrap()
                        .remove("version");
                }
                reply(&mut server, &final_query, workspace(vec![final_document])).await;
                server
            });
            let mut args = arguments();
            args.path = Some(name.into());
            let error = replacements(&mut client, &fixture.root, &args, &context)
                .await
                .err()
                .unwrap();
            let expected = if missing_or_null {
                "version is missing or null"
            } else {
                "version is unknown or stale"
            };
            assert!(error.contains(expected), "{error}");
            let _server = fake.await.unwrap();
            assert_eq!(std::fs::read_to_string(path).unwrap(), "old");
        }
    }
}

#[tokio::test]
async fn concrete_typescript_client_rejects_mixed_shapes_and_annotations_in_either_phase() {
    for final_phase in [false, true] {
        for violation in ["mixed", "mixed_null", "annotations", "annotated_edit"] {
            let fixture = Fixture::new();
            let path = fixture.write("a.ts", "old");
            let (mut client, context, mut server) = prepared(&fixture).await;
            let raw_uri = context.document.uri.clone();
            let fake = tokio::spawn(async move {
                let edits = vec![edit_at((0, 0), (0, 3), "new")];
                let native = json!({"changes":{(raw_uri.clone()):edits.clone()}});
                let mut invalid = native.clone();
                match violation {
                    "mixed" => {
                        invalid["documentChanges"] = json!([document(&raw_uri, json!(1), edits)])
                    }
                    "mixed_null" => invalid["documentChanges"] = Value::Null,
                    "annotations" => invalid["changeAnnotations"] = json!({}),
                    "annotated_edit" => {
                        invalid["changes"][&raw_uri][0]["annotationId"] = json!("confirm")
                    }
                    _ => unreachable!(),
                }
                let discovery = server.next().await;
                if final_phase {
                    reply(&mut server, &discovery, native).await;
                    let (final_query, _) = final_request(&mut server).await;
                    reply(&mut server, &final_query, invalid).await;
                } else {
                    reply(&mut server, &discovery, invalid).await;
                }
                server
            });
            assert!(
                replacements(&mut client, &fixture.root, &arguments(), &context)
                    .await
                    .is_err(),
                "{violation}"
            );
            let _server = fake.await.unwrap();
            assert_eq!(std::fs::read_to_string(path).unwrap(), "old");
        }
    }
}

#[tokio::test]
async fn cancellation_and_deadline_in_either_rename_phase_drop_owned_transport() {
    for final_phase in [false, true] {
        for deadline in [false, true] {
            let fixture = Fixture::new();
            fixture.write("a.ts", "old");
            let (mut client, context, mut server) = prepared(&fixture).await;
            let raw_uri = context.document.uri.clone();
            let root = fixture.root.clone();
            // Own the client just like the outer LSP lease: no client remains
            // cached on cancellation or deadline, and no preview can be stored.
            let task = tokio::spawn(async move {
                let args = arguments();
                let query = async move { replacements(&mut client, &root, &args, &context).await };
                tokio::time::timeout(Duration::from_secs(1), query).await
            });
            let first = server.next().await;
            if final_phase {
                reply(
                    &mut server,
                    &first,
                    workspace(vec![document(
                        &raw_uri,
                        json!(1),
                        vec![edit_at((0, 0), (0, 3), "new")],
                    )]),
                )
                .await;
                let _ = final_request(&mut server).await;
            }
            if deadline {
                assert!(task.await.unwrap().is_err());
            } else {
                task.abort();
                assert!(matches!(task.await, Err(error) if error.is_cancelled()));
            }
            assert!(
                server.closed().await,
                "owned transport must close on cancellation/deadline"
            );
            assert_eq!(
                std::fs::read_to_string(fixture.root.join("a.ts")).unwrap(),
                "old"
            );
        }
    }
}

#[tokio::test]
async fn concrete_client_rejects_unpaired_utf16_surrogates_in_server_json() {
    let fixture = Fixture::new();
    fixture.write("a.ts", "old");
    let (mut client, context, mut server) = prepared(&fixture).await;
    let raw_uri = context.document.uri.clone();
    let fake = tokio::spawn(async move {
        let first = server.next().await;
        reply(
            &mut server,
            &first,
            workspace(vec![document(
                &raw_uri,
                json!(1),
                vec![edit_at((0, 0), (0, 3), "new")],
            )]),
        )
        .await;
        let (final_query, versions) = final_request(&mut server).await;
        let encoded_uri = serde_json::to_string(&raw_uri).unwrap();
        let invalid = format!(
            r#"{{"jsonrpc":"2.0","id":{},"result":{{"documentChanges":[{{"textDocument":{{"uri":{},"version":{}}},"edits":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":3}}}},"newText":"\ud800"}}]}}]}}}}"#,
            final_query["id"], encoded_uri, versions[&raw_uri],
        );
        server.send_raw(invalid.as_bytes()).await;
        server
    });
    assert!(
        replacements(&mut client, &fixture.root, &arguments(), &context)
            .await
            .is_err()
    );
    let _server = fake.await.unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("a.ts")).unwrap(),
        "old"
    );
}

#[test]
fn rename_file_count_edit_count_and_combined_snapshot_budgets_are_enforced() {
    let fixture = Fixture::new();
    let mut documents = Vec::new();
    for index in 0..=MAX_FILES {
        let path = fixture.write(&format!("{index}.ts"), "old");
        documents.push(document(
            &uri(&path).unwrap(),
            json!(7),
            vec![edit_at((0, 0), (0, 3), "new")],
        ));
    }
    for native_changes in [false, true] {
        assert!(
            targets_for_language(
                &fixture.root,
                workspace_for_shape(documents.clone(), native_changes),
                Language::TypeScript
            )
            .err()
            .unwrap()
            .contains("32")
        );
    }
    let a = fixture.write("a.ts", "old");
    let excessive = vec![document(
        &uri(&a).unwrap(),
        json!(7),
        vec![edit_at((0, 0), (0, 0), "x"); MAX_TEXT_EDITS + 1],
    )];
    for native_changes in [false, true] {
        assert!(
            targets_for_language(
                &fixture.root,
                workspace_for_shape(excessive.clone(), native_changes),
                Language::TypeScript
            )
            .is_err()
        );
    }
    let text = "a".repeat(512 * 1024);
    let mut baselines = BTreeMap::new();
    let mut documents = Vec::new();
    for index in 0..5 {
        let base = baseline(&fixture, &format!("large-{index}.ts"), &text);
        documents.push(document(
            &base.document.uri,
            json!(7),
            vec![edit_at((0, 0), (0, 1), "b")],
        ));
        baselines.insert(base.document.path.clone(), base);
    }
    for native_changes in [false, true] {
        let parsed = targets_for_language(
            &fixture.root,
            workspace_for_shape(documents.clone(), native_changes),
            Language::TypeScript,
        )
        .unwrap();
        assert!(
            convert(&fixture.root, parsed, &baselines)
                .err()
                .unwrap()
                .contains("4 MiB")
        );
    }
}

#[test]
fn rename_arguments_require_new_name_and_refuse_truncation_or_names_with_nul() {
    for name in [
        None,
        Some(String::new()),
        Some("bad\0name".into()),
        Some("a".repeat(MAX_NEW_NAME_BYTES + 1)),
    ] {
        let mut args = arguments();
        args.new_name = name;
        assert!(validate_arguments(&args).is_err());
    }
    let mut args = arguments();
    args.limit = Some(1);
    assert!(validate_arguments(&args).is_err());
    args.limit = None;
    args.operation = Operation::Definition;
    assert!(validate_arguments(&args).is_err());
    assert!(validate_arguments(&arguments()).is_ok());
}
