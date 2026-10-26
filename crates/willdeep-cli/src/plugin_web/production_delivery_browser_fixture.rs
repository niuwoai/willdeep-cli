//! Manual browser fixture: real bundled React UI and the real file streaming
//! implementation, fixed completion metadata. This does NOT test paid creation,
//! Runtime scheduling, or the independent Runtime/domain authorization gates.
use super::*;

#[tokio::test]
#[ignore = "manual browser fixture; set PRODUCTION_BROWSER_FIXTURE_ROOT explicitly"]
async fn production_browser_fixture() {
    let root = PathBuf::from(
        std::env::var("PRODUCTION_BROWSER_FIXTURE_ROOT").expect("explicit fixture root"),
    )
    .canonicalize()
    .unwrap();
    let ui = root.join("../../../ui/dist").canonicalize().unwrap();
    let source = root.join("fixture-only-composed-episode.mp4");
    assert!(source.is_file(), "run the local composition fixture first");
    let id = "11111111-1111-4111-8111-111111111111";
    let sha = format!("{:x}", Sha256::digest(std::fs::read(&source).unwrap()));
    let film = FileProof {
        filename: "fixture-only-browser-episode.mp4".into(),
        file_path: root.join("fixture-only-browser-episode.mp4"),
        sha256: sha.clone(),
        bytes: source.metadata().unwrap().len(),
        mime_type: "video/mp4".into(),
    };
    std::fs::copy(source, &film.file_path).unwrap();
    let mut files = BTreeMap::new();
    files.insert("episode-1".to_owned(), film.clone());
    for selector in ["manifest", "records"] {
        let bytes = serde_json::to_vec_pretty(&json!({"fixtureOnly":true,"workflowID":id,"purpose":"browser transport only","filmSHA256":sha})).unwrap();
        let filename = format!("fixture-only-browser-{selector}.json");
        let file_path = root.join(&filename);
        std::fs::write(&file_path, &bytes).unwrap();
        files.insert(
            selector.to_owned(),
            FileProof {
                filename,
                file_path,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes: bytes.len() as u64,
                mime_type: "application/json".into(),
            },
        );
    }
    let descriptor = |selector: &str| {
        let file = &files[selector];
        json!({"selector":selector,"filename":file.filename,"sha256":file.sha256,"bytes":file.bytes,"mimeType":file.mime_type,
            "url":format!("/plugin-production/{id}/{selector}"),"episodeOrder":1,"durationMs":30000})
    };
    let delivery = json!({"ok":true,"manifest":descriptor("manifest"),"records":descriptor("records"),"episodes":[descriptor("episode-1")]});
    let limits: BTreeMap<_, _> = [
        "model_calls",
        "tokens",
        "images",
        "tts_characters",
        "music_calls",
        "music_seconds",
        "video_calls",
        "video_seconds",
    ]
    .into_iter()
    .map(|unit| (unit, 1000))
    .collect();
    let listing = json!({"ok":true,"workspaces":["/fixture-only"],"defaults":{"episodeCount":40,"episodeSeconds":90,"aspect":"portrait","requiresMediaBudget":true},"workflows":[{
        "id":id,"status":"completed","cancelRequested":false,"input":{"brief":"固定素材浏览器交付验收，不是真实创作","workspace":"/fixture-only","spec":{"episode_count":1,"episode_seconds":30,"aspect":"portrait"}},
        "usage":{"limits":limits,"used":{"tokens":100},"reserved":{},"remaining":{},"unknown_requests":[]},"completedNodes":24,"totalNodes":24,"episodes":[],
        "progress":{"title":"固定素材浏览器交付验收","written":1,"revisions":1,"issues":[]}}]});
    let workflow = listing["workflows"][0].clone();
    let bridge = format!(
        r#"<script>window.willdeep={{getContext:()=>({{locale:'zh-Hans'}}),storage:{{get:async()=>({{value:null}}),set:async()=>({{ok:true}})}},executeCommand:async(name,args)=>JSON.stringify(name==='production.list'?{listing}:name==='production.inspect'?{{ok:true,workflow:{workflow}}}:name==='production.delivery'?{delivery}:name==='drama.list'?{{ok:true,dramas:[]}}:{{ok:true,jobs:[],skills:[]}})}};</script>"#
    );
    let index = std::fs::read_to_string(ui.join("index.html"))
        .unwrap()
        .replace("<head>", &format!("<head>{bridge}"));
    let files = Arc::new(files);
    let downloads_root = root.clone();
    let app = Router::new()
        .route("/", get(|| async { axum::response::Html("<html><body style='margin:0'><iframe title='fixture-only' src='/plugin-page' sandbox='allow-scripts allow-downloads' style='width:100vw;height:100vh;border:0'></iframe></body></html>") }))
        .route("/plugin-page", get(move |headers:HeaderMap| {let html=index.clone(); async move {html_response(html,&headers)}}))
        .route("/assets/{name}",get(move |Path(name):Path<String>,headers:HeaderMap| {let ui=ui.clone();async move {
            if name.contains(['/', '\\']) {return Err(invalid());}
            let bytes=std::fs::read(ui.join("assets").join(&name)).map_err(|_|invalid())?;
            let mut response=([(header::CONTENT_TYPE,mime_for(&name))],bytes).into_response();apply_sandbox_cors(&mut response,&headers);Ok::<_,PluginWebError>(response)
        }}))
        .route("/plugin-production/{workflow}/{selector}",get(move |Path((workflow,selector)):Path<(String,String)>,Query(options):Query<Options>,headers:HeaderMap| {
            let files=files.clone();let root=downloads_root.clone();async move {
                if workflow!=id{return Err(invalid());}
                let proof=files.get(&selector).cloned().ok_or_else(invalid)?;
                let (file,proof)=verified_file(&root,proof)?;stream_file(file,proof,options.download,&headers)
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    println!(
        "PRODUCTION_BROWSER_FIXTURE_URL=http://{}/",
        listener.local_addr().unwrap()
    );
    use std::io::Write;
    std::io::stdout().flush().unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}
