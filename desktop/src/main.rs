#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod service;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::menu::{AboutMetadata, Menu, MenuItem, SubmenuBuilder};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

use service::Service;

const APP_NAME: &str = "WillDeep Desktop";
const MAIN: &str = "main";
const OPEN_FOLDER: &str = "open-folder";
const NEW_SESSION: &str = "new-session";
const TOGGLE_CHANGES: &str = "toggle-changes";
const PICKER_ROUTE: &str = "/__willdeep_native/open-folder";

#[derive(Default)]
struct Desktop {
    service: Mutex<Option<Service>>,
    origin: OnceLock<Url>,
    window_count: AtomicUsize,
}

fn main() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Desktop::default())
        .menu(menu)
        .on_menu_event(|app, event| native_action(app, event.id().as_ref()))
        .setup(|app| {
            let handle = app.handle().clone();
            build_window(&handle, MAIN, WebviewUrl::App("index.html".into()))?;
            std::thread::spawn(move || start_web_service(&handle));
            Ok(())
        })
        .build(tauri::generate_context!());
    match result {
        Ok(app) => app.run(|handle, event| match event {
            RunEvent::Exit => {
                if let Ok(mut service) = handle.state::<Desktop>().service.lock() {
                    service.take();
                }
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } => reopen(handle),
            _ => {}
        }),
        Err(error) => {
            eprintln!("Could not start {APP_NAME}: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(target_os = "macos")]
fn reopen(handle: &AppHandle) {
    if let Some(window) = handle.get_webview_window(MAIN) {
        let _ = window.show();
        let _ = window.set_focus();
    } else if let Some(mut url) = handle.state::<Desktop>().origin.get().cloned() {
        url.set_query(Some("desktop=1"));
        if let Err(error) = build_window(handle, MAIN, WebviewUrl::External(url)) {
            eprintln!("Could not reopen desktop window: {error}");
        }
    }
}
fn start_web_service(handle: &AppHandle) {
    let service = Service::start();
    let main = handle.clone();
    if let Err(error) = handle.run_on_main_thread(move || show_service(&main, service)) {
        eprintln!("Could not open desktop Web service: {error}");
    }
}

fn show_service(handle: &AppHandle, service: std::io::Result<Service>) {
    let Some(window) = handle.get_webview_window(MAIN) else {
        return;
    };
    let desktop = handle.state::<Desktop>();
    if let Ok(service) = service {
        if let Ok(mut slot) = desktop.service.lock() {
            let url = service.url.clone();
            let _ = desktop
                .origin
                .set(url.join("/").expect("valid local service URL"));
            *slot = Some(service);
            if window.navigate(url).is_ok() {
                return;
            }
        }
    }
    let _ = window.eval("window.willdeepStartupFailed()");
}

fn build_window(app: &AppHandle, label: &str, url: WebviewUrl) -> tauri::Result<WebviewWindow> {
    let navigation = app.clone();
    let origin_window = label.to_owned();
    let new_window = app.clone();
    WebviewWindowBuilder::new(app, label, url)
        .title(APP_NAME)
        .inner_size(1380.0, 900.0)
        .min_inner_size(650.0, 540.0)
        .on_navigation(move |url| can_navigate(&navigation, &origin_window, url))
        .on_new_window(move |url, _features| {
            let handle = new_window.clone();
            if trusted(&handle, &url) {
                std::thread::spawn(move || open_conversation_window(&handle, url));
            } else {
                open_external(&url);
            }
            NewWindowResponse::Deny
        })
        .build()
}

fn trusted(app: &AppHandle, url: &Url) -> bool {
    app.state::<Desktop>()
        .origin
        .get()
        .is_some_and(|origin| origin.origin() == url.origin())
}

fn can_navigate(app: &AppHandle, label: &str, url: &Url) -> bool {
    if trusted(app, url) {
        if url.path() == PICKER_ROUTE {
            let handle = app.clone();
            let label = label.to_owned();
            let _ = app.run_on_main_thread(move || {
                if let Some(window) = handle.get_webview_window(&label) {
                    select_folder(window);
                }
            });
            return false;
        }
        return true;
    }
    if matches!(url.scheme(), "tauri" | "http" | "https")
        && matches!(url.host_str(), Some("localhost" | "tauri.localhost"))
        && app.state::<Desktop>().origin.get().is_none()
    {
        return true;
    }
    open_external(url);
    false
}

fn open_external(url: &Url) {
    if matches!(url.scheme(), "http" | "https")
        && url.host_str() != Some("tauri.localhost")
        && let Err(error) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>)
    {
        eprintln!("Could not open external link: {error}");
    }
}

fn open_conversation_window(app: &AppHandle, url: Url) {
    let handle = app.clone();
    let result = app.run_on_main_thread(move || {
        let id = handle
            .state::<Desktop>()
            .window_count
            .fetch_add(1, Ordering::Relaxed);
        if let Err(error) = build_window(
            &handle,
            &format!("conversation-{id}"),
            WebviewUrl::External(url),
        ) {
            eprintln!("Could not open conversation window: {error}");
        }
    });
    if let Err(error) = result {
        eprintln!("Could not dispatch conversation window: {error}");
    }
}

fn native_action(app: &AppHandle, action: &str) {
    let windows = app.webview_windows();
    let Some(window) = windows
        .values()
        .find(|window| window.is_focused().unwrap_or(false))
        .or_else(|| windows.get(MAIN))
        .cloned()
    else {
        return;
    };
    if !window.url().is_ok_and(|url| trusted(app, &url)) {
        return;
    }
    match action {
        OPEN_FOLDER => select_folder(window),
        NEW_SESSION => dispatch(
            &window,
            "willdeep-desktop-new-session",
            serde_json::Value::Null,
        ),
        TOGGLE_CHANGES => dispatch(
            &window,
            "willdeep-desktop-toggle-changes",
            serde_json::Value::Null,
        ),
        _ => {}
    }
}

fn select_folder(window: WebviewWindow) {
    window
        .dialog()
        .file()
        .set_parent(&window)
        .pick_folder(move |picked| {
            let Some(path) = picked.and_then(|path| path.into_path().ok()) else {
                return;
            };
            let Some(path) = path.to_str() else { return };
            dispatch(
                &window,
                "willdeep-desktop-workspace",
                serde_json::json!(path),
            );
        });
}

fn dispatch(window: &WebviewWindow, event: &str, detail: serde_json::Value) {
    let name = serde_json::json!(event);
    if let Err(error) = window.eval(format!(
        "window.dispatchEvent(new CustomEvent({name}, {{ detail: {detail} }}));"
    )) {
        eprintln!("Could not dispatch desktop action: {error}");
    }
}

fn menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(
        app,
        OPEN_FOLDER,
        "Open Workspace…",
        true,
        Some("CmdOrCtrl+O"),
    )?;
    let new = MenuItem::with_id(app, NEW_SESSION, "New Session", true, Some("CmdOrCtrl+N"))?;
    let changes = MenuItem::with_id(
        app,
        TOGGLE_CHANGES,
        "Workspace Changes",
        true,
        Some("CmdOrCtrl+Shift+B"),
    )?;
    let application = SubmenuBuilder::new(app, APP_NAME)
        .about(Some(AboutMetadata {
            name: Some(APP_NAME.into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            ..Default::default()
        }))
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&open)
        .item(&new)
        .separator()
        .close_window()
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View").item(&changes).build()?;
    Menu::with_items(app, &[&application, &file, &edit, &view])
}
