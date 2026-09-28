use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
};

use base64::Engine;
use notify::{RecursiveMode, Watcher};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Window, WindowBuilder},
};
use wry::{http::Request, http::Response, WebView, WebViewBuilder, WebViewBuilderExtWindows};

const INDEX: &[u8] = include_bytes!("../assets/index.html");
const MARKED: &[u8] = include_bytes!("../assets/marked.min.js");
const HIGHLIGHT: &[u8] = include_bytes!("../assets/highlight.min.js");
const MERMAID: &[u8] = include_bytes!("../assets/mermaid.min.js");


fn asset(path: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match path {
        "/" | "/index.html" => (INDEX, "text/html; charset=utf-8"),
        "/assets/marked.min.js" => (MARKED, "text/javascript"),
        "/assets/highlight.min.js" => (HIGHLIGHT, "text/javascript"),
        "/assets/mermaid.min.js" => (MERMAID, "text/javascript"),

        _ => return None,
    })
}

#[derive(Debug)]
enum UserEvent {
    PageReady,
    Reload,
    OpenExplorer,
    CopyPath,
    CopySource,
    EditNotepad,
    ToggleOnTop,
}

fn open_in_explorer(path: &Path) {
    let _ = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

fn copy_to_clipboard(text: &str) {
    if let Ok(mut c) = std::process::Command::new("clip.exe")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        use std::io::Write;
        let _ = c.stdin.as_mut().map(|s| s.write_all(text.as_bytes()));
        let _ = c.wait();
    }
}

fn open_in_notepad(path: &Path) {
    let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
}

fn load_and_render(webview: &WebView, window: &Window, path: &Path) {
    match fs::read(path) {
        Ok(bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let raw_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let js_name = raw_name.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = webview.evaluate_script(&format!("render('{}', '{}')", b64, js_name));
            window.set_title(&format!("mdpeek - {}", raw_name));
        }
        Err(e) => {
            let _ = webview.evaluate_script(&format!(
                "document.getElementById('content').innerHTML='<pre>{}</pre>'",
                e.to_string().replace('\'', "\\'")
            ));
        }
    }
}

fn main() {
    let mut current_file = std::env::args().nth(1).map(PathBuf::from);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("mdpeek")
        .with_inner_size(tao::dpi::LogicalSize::new(1000.0, 800.0))
        .build(&event_loop)
        .expect("create window");

    let ipc_proxy = proxy.clone();
    let webview = WebViewBuilder::new()
        .with_url("http://mdpeek.localhost/")
        .with_default_context_menus(false)
        .with_custom_protocol("mdpeek".into(), move |_id, req: Request<Vec<u8>>| {
            match asset(req.uri().path()) {
                Some((body, mime)) => Response::builder()
                    .header("content-type", mime)
                    .header("cache-control", "no-store")
                    .body(Cow::Borrowed(body))
                    .unwrap(),
                None => Response::builder()
                    .status(404)
                    .body(Cow::Borrowed(&b""[..]))
                    .unwrap(),
            }
        })
        .with_ipc_handler(move |req| {
            let ev = match req.body().as_str() {
                "ready" => Some(UserEvent::PageReady),
                "reload" => Some(UserEvent::Reload),
                "open-explorer" => Some(UserEvent::OpenExplorer),
                "copy-path" => Some(UserEvent::CopyPath),
                "copy-source" => Some(UserEvent::CopySource),
                "edit-notepad" => Some(UserEvent::EditNotepad),
                "toggle-ontop" => Some(UserEvent::ToggleOnTop),
                _ => None,
            };
            if let Some(ev) = ev {
                let _ = ipc_proxy.send_event(ev);
            }
        })
        .build(&window)
        .expect("create webview");

    // watch the file's parent dir so editor atomic-saves (replace) are seen;
    // rebuilt whenever the current file changes (e.g. after drag & drop)
    let mut _watcher: Option<notify::RecommendedWatcher> = None;
    let mut on_top = false;
    let setup_watch = move |path: &Path, watcher: &mut Option<notify::RecommendedWatcher>| {
        *watcher = None;
        let Ok(f) = path.canonicalize() else { return };
        let Some(parent) = f.parent().map(|p| p.to_path_buf()) else { return };
        let watch_proxy = proxy.clone();
        let fname = f.file_name().map(|n| n.to_os_string());
        if let Ok(mut w) = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let hit = ev.paths.iter().any(|p| {
                    p.file_name()
                        .map(|n| Some(n.to_os_string()) == fname.clone())
                        .unwrap_or(false)
                });
                if hit {
                    let _ = watch_proxy.send_event(UserEvent::Reload);
                }
            }
        }) {
            if w.watch(&parent, RecursiveMode::NonRecursive).is_ok() {
                *watcher = Some(w);
            }
        }
    };
    if let Some(f) = current_file.clone() {
        setup_watch(&f, &mut _watcher);
    }

    event_loop.run(move |event, _elwt, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::Focused(true) => {
                    let _ = webview.focus();
                }
                WindowEvent::DroppedFile(path) => {
                    current_file = Some(path.clone());
                    setup_watch(&path, &mut _watcher);
                    load_and_render(&webview, &window, &path);
                }
                _ => {}
            },
            Event::UserEvent(ev) => match ev {
                UserEvent::PageReady => {
                    let _ = webview.focus();
                    if let Some(f) = current_file.clone() {
                        load_and_render(&webview, &window, &f);
                    }
                }
                UserEvent::ToggleOnTop => {
                    on_top = !on_top;
                    window.set_always_on_top(on_top);
                }
                _ => {
                    if let Some(f) = current_file.clone() {
                        match ev {
                            UserEvent::OpenExplorer => open_in_explorer(&f),
                            UserEvent::CopyPath => copy_to_clipboard(&f.to_string_lossy()),
                            UserEvent::CopySource => {
                                if let Ok(bytes) = fs::read(&f) {
                                    copy_to_clipboard(&String::from_utf8_lossy(&bytes));
                                }
                            }
                            UserEvent::EditNotepad => open_in_notepad(&f),
                            _ => load_and_render(&webview, &window, &f),
                        }
                    }
                }
            },
            _ => {}
        }
    });
}
