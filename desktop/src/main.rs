#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod service;

use std::sync::{Mutex, OnceLock};

use tauri::menu::{AboutMetadata, Menu, MenuItem, SubmenuBuilder};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;

use service::{Notice, Service};

const APP_NAME: &str = "WillDeep Desktop";
const MAIN: &str = "main";
const OPEN_FOLDER: &str = "open-folder";
const NEW_SESSION: &str = "new-session";
const TOGGLE_CHANGES: &str = "toggle-changes";
const PICKER_ROUTE: &str = "/__willdeep_native/open-folder";
const DESKTOP_MARKER: &str = "Object.defineProperty(window, '__willdeepDesktop', { value: true });";

#[derive(Default)]
struct Desktop {
    service: Mutex<Option<Service>>,
    origin: OnceLock<Url>,
}

fn main() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
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
    if let Ok(mut service) = service
        && let Ok(mut slot) = desktop.service.lock()
    {
        let url = service.url.clone();
        let _ = desktop
            .origin
            .set(url.join("/").expect("valid local service URL"));
        if let Some(notices) = service.notices.take() {
            watch_notices(handle, notices);
        }
        *slot = Some(service);
        if window.navigate(url).is_ok() {
            return;
        }
    }
    let _ = window.eval("window.willdeepStartupFailed()");
}

/// The adapter's stdout is read on its own thread. This one only forwards each
/// notice onto the main thread, which is where the windowing system is talked to.
fn watch_notices(handle: &AppHandle, notices: std::sync::mpsc::Receiver<Notice>) {
    let handle = handle.clone();
    std::thread::spawn(move || {
        while let Ok(notice) = notices.recv() {
            let posted = handle.clone();
            if handle
                .run_on_main_thread(move || post_notice(&posted, &notice))
                .is_err()
            {
                break;
            }
        }
    });
}

fn post_notice(handle: &AppHandle, notice: &Notice) {
    let mut builder = handle.notification().builder().body(&notice.body);
    if !notice.title.is_empty() {
        builder = builder.title(&notice.title);
    }
    if let Err(error) = builder.show() {
        eprintln!("Could not post desktop notification: {error}");
    }
}

fn build_window(app: &AppHandle, label: &str, url: WebviewUrl) -> tauri::Result<WebviewWindow> {
    let navigation = app.clone();
    let origin_window = label.to_owned();
    let popup = app.clone();
    let popup_window = label.to_owned();
    WebviewWindowBuilder::new(app, label, url)
        .title(APP_NAME)
        .inner_size(1380.0, 900.0)
        .min_inner_size(650.0, 540.0)
        .initialization_script(DESKTOP_MARKER)
        .on_navigation(move |url| can_navigate(&navigation, &origin_window, url))
        .on_new_window(move |url, _features| {
            match destination(popup.state::<Desktop>().origin.get(), &url) {
                Destination::CurrentWindow => {
                    let handle = popup.clone();
                    let label = popup_window.clone();
                    // Defer until the WebView has returned from its popup callback.
                    std::thread::spawn(move || navigate_current_window(&handle, &label, url));
                }
                Destination::ExternalBrowser => open_external(&url),
                Destination::Blocked => {}
            }
            NewWindowResponse::Deny
        })
        .build()
}

#[derive(Debug, PartialEq, Eq)]
enum Destination {
    CurrentWindow,
    ExternalBrowser,
    Blocked,
}

fn destination(origin: Option<&Url>, url: &Url) -> Destination {
    if !matches!(url.scheme(), "http" | "https") || url.host_str() == Some("tauri.localhost") {
        Destination::Blocked
    } else if origin.is_some_and(|origin| origin.origin() == url.origin()) {
        Destination::CurrentWindow
    } else {
        Destination::ExternalBrowser
    }
}

fn can_navigate(app: &AppHandle, label: &str, url: &Url) -> bool {
    let desktop = app.state::<Desktop>();
    let origin = desktop.origin.get();
    if origin.is_none()
        && matches!(url.scheme(), "tauri" | "http" | "https")
        && matches!(url.host_str(), Some("localhost" | "tauri.localhost"))
    {
        return true;
    }
    match destination(origin, url) {
        Destination::CurrentWindow if url.path() == PICKER_ROUTE => {
            let handle = app.clone();
            let label = label.to_owned();
            let _ = app.run_on_main_thread(move || {
                if let Some(window) = handle.get_webview_window(&label) {
                    select_folder(window);
                }
            });
            false
        }
        Destination::CurrentWindow => true,
        Destination::ExternalBrowser => {
            open_external(url);
            false
        }
        Destination::Blocked => false,
    }
}

fn open_external(url: &Url) {
    if let Err(error) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
        eprintln!("Could not open external link: {error}");
    }
}

fn navigate_current_window(app: &AppHandle, label: &str, url: Url) {
    if let Some(window) = app.get_webview_window(label) {
        // Navigate from the document to keep the request same-site for the Strict cookie.
        let target = serde_json::json!(url.as_str());
        if let Err(error) = window.eval(format!("window.location.assign({target});")) {
            eprintln!("Could not navigate the current desktop window: {error}");
        }
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
    let desktop = app.state::<Desktop>();
    if !window
        .url()
        .is_ok_and(|url| destination(desktop.origin.get(), &url) == Destination::CurrentWindow)
    {
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

#[cfg(test)]
mod tests {
    use super::{Destination, Url, destination};

    #[test]
    fn navigation_keeps_only_the_desktop_service_in_the_current_window() {
        let origin = Url::parse("http://127.0.0.1:12345/").unwrap();
        for (link, expected) in [
            ("/?session=other#turn", Destination::CurrentWindow),
            ("/share/example", Destination::CurrentWindow),
            ("/__willdeep_native/open-folder", Destination::CurrentWindow),
            ("https://example.com/docs", Destination::ExternalBrowser),
            ("http://127.0.0.1:54321/", Destination::ExternalBrowser),
            ("https://127.0.0.1:12345/", Destination::ExternalBrowser),
            ("http://localhost:12345/", Destination::ExternalBrowser),
            ("http://tauri.localhost/", Destination::Blocked),
            ("tauri://localhost/", Destination::Blocked),
            ("about:blank", Destination::Blocked),
            ("blob:http://127.0.0.1:12345/example", Destination::Blocked),
            ("file:///tmp/example", Destination::Blocked),
        ] {
            let url = origin.join(link).unwrap();
            assert_eq!(destination(Some(&origin), &url), expected, "{link}");
        }
        assert_eq!(destination(None, &origin), Destination::ExternalBrowser);
    }
}
