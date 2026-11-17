use super::super::tests::Fixture;
use super::*;

fn range(start: u32, end: u32) -> Value {
    json!({"start":{"line":0,"character":start},"end":{"line":0,"character":end}})
}

fn payload(raw: Value) -> QueryPayload {
    QueryPayload {
        raw,
        diagnostics_mode: None,
        published_version: None,
        version_verified: true,
        truncated: false,
    }
}

#[tokio::test]
async fn locations_filter_external_schemes_symlinks_and_invalid_utf16() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let context = fixture.context("a.ts", "const 😀name = 1;\n");
    let b = fixture.write("b.ts", "export const name = 1;\n");
    let external = outside.write("outside.ts", "const outside = 1;\n");
    let locations = vec![
        json!({"uri":uri(&b).unwrap(),"range":range(0,6)}),
        json!({"targetUri":uri(&b).unwrap(),"targetRange":range(0,6),"targetSelectionRange":range(0,6)}),
        json!({"uri":uri(&external).unwrap(),"range":range(0,1)}),
        json!({"uri":"https://example.test/outside.ts","range":range(0,1)}),
        json!({"uri":context.document.uri,"range":range(7,8)}), // surrogate split
        json!({"uri":context.document.uri,"range":range(9,8)}), // backwards range
        json!({"uri":format!("{}?query=1",context.document.uri),"range":range(0,1)}),
    ];
    #[cfg(unix)]
    let locations = {
        std::os::unix::fs::symlink(&external, fixture.root.join("escape.ts")).unwrap();
        let mut locations = locations;
        locations
            .push(json!({"uri":uri(&fixture.root.join("escape.ts")).unwrap(),"range":range(0,1)}));
        locations
    };
    let rendered = render(
        &fixture.root,
        &context,
        Operation::Definition,
        20,
        payload(json!(locations)),
    )
    .await
    .unwrap();
    let value: Value = serde_json::from_str(&rendered.text).unwrap();
    assert_eq!(value["results"].as_array().unwrap().len(), 2);
    assert!(value["filtered_results"].as_u64().unwrap() >= 5);
    for result in value["results"].as_array().unwrap() {
        assert_eq!(result["path"], "b.ts");
        assert_eq!(result["version"], snapshot(b"export const name = 1;\n"));
        assert!(result["lsp_version"].is_null());
    }
    assert!(!rendered.text.contains("outside.ts"));
}

#[tokio::test]
async fn symbols_are_flattened_and_limited_without_raw_server_fields() {
    let fixture = Fixture::new();
    let context = fixture.context("a.rs", "fn hello() {}\n");
    let item = json!({"name":"hello","kind":12,"range":range(0,13),"selectionRange":range(3,8),
    "data":{"command":"do-not-expose"},"children":[
        {"name":"child","kind":13,"range":range(0,13),"selectionRange":range(3,8)}
    ]});
    let rendered = render(
        &fixture.root,
        &context,
        Operation::Symbols,
        1,
        payload(json!([item])),
    )
    .await
    .unwrap();
    let value: Value = serde_json::from_str(&rendered.text).unwrap();
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["results"][0]["name"], "hello");
    assert_eq!(value["truncated"], true);
    assert!(!rendered.text.contains("do-not-expose"));
}

#[tokio::test]
async fn unversioned_or_filtered_diagnostics_never_claim_a_clean_snapshot() {
    let fixture = Fixture::new();
    let context = fixture.context("a.rs", "fn a() {}\n");
    for (verified, items) in [
        (false, json!([])),
        (true, json!([{"range":range(999,1000),"message":"invalid"}])),
    ] {
        let payload = QueryPayload {
            raw: items,
            diagnostics_mode: Some("push"),
            published_version: None,
            version_verified: verified,
            truncated: false,
        };
        let value: Value = serde_json::from_str(
            &render(&fixture.root, &context, Operation::Diagnostics, 20, payload)
                .await
                .unwrap()
                .text,
        )
        .unwrap();
        assert_eq!(value["diagnostics"]["complete"], false);
        assert!(value["diagnostics"]["clean"].is_null());
    }
    let empty = QueryPayload {
        raw: json!([]),
        diagnostics_mode: Some("pull"),
        published_version: None,
        version_verified: true,
        truncated: false,
    };
    let value: Value = serde_json::from_str(
        &render(&fixture.root, &context, Operation::Diagnostics, 20, empty)
            .await
            .unwrap()
            .text,
    )
    .unwrap();
    assert_eq!(value["diagnostics"]["complete"], true);
    assert_eq!(value["diagnostics"]["clean"], true);
}

#[tokio::test]
async fn diagnostic_output_is_bounded_and_omits_related_external_locations() {
    let fixture = Fixture::new();
    let context = fixture.context("a.rs", "fn a() {}\n");
    let diagnostic = json!({"range":range(0,2),"message":"message".repeat(500),"severity":1,
        "relatedInformation":[{"location":{"uri":"file:///secret.rs","range":range(0,1)},"message":"outside"}],
        "data":{"secret":"not-output"}});
    let mut query = payload(json!(vec![diagnostic; 200]));
    query.diagnostics_mode = Some("pull");
    let rendered = render(&fixture.root, &context, Operation::Diagnostics, 200, query)
        .await
        .unwrap();
    assert!(rendered.text.len() <= MAX_OUTPUT_BYTES);
    let value: Value = serde_json::from_str(&rendered.text).unwrap();
    assert_eq!(value["truncated"], true);
    assert_eq!(value["diagnostics"]["clean"], false);
    assert_eq!(value["diagnostics"]["complete"], false);
    assert!(!rendered.text.contains("secret.rs"));
    assert!(!rendered.text.contains("not-output"));
}

#[tokio::test]
async fn changes_to_returned_sources_invalidate_the_normalized_result() {
    let fixture = Fixture::new();
    let context = fixture.context("a.rs", "fn a() {}\n");
    let b = fixture.write("b.rs", "fn b() {}\n");
    let rendered = render(
        &fixture.root,
        &context,
        Operation::References,
        10,
        payload(json!([{"uri":uri(&b).unwrap(),"range":range(3,4)}])),
    )
    .await
    .unwrap();
    fixture.write("b.rs", "fn newer() {}\n");
    assert!(
        super::super::client::verify_snapshots(&fixture.root, &rendered.checked_documents)
            .await
            .is_err()
    );
}
