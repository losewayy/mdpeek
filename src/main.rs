// release 构建使用 GUI 子系统，否则双击时会弹出一个控制台黑框；
// debug 构建保留控制台，方便看 println! / panic 输出
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    borrow::Cow,
    fs,
    path::{Component, Path, PathBuf},
    sync::{LazyLock, Mutex},
};

use base64::Engine;
use notify::{RecursiveMode, Watcher};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Icon, Window, WindowBuilder},
};
use wry::{
    http::Request, http::Response, NewWindowResponse, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows,
};

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
    Refocus,
    Save(String),
    /// 预览器里点到不该由自己承载的链接，交给系统默认程序打开
    OpenUrl(String),
    /// 编辑器缓冲区是否有未保存改动，用来防止拖入新文件时换掉保存目标
    EditorDirty(bool),
    /// 用户在确认框里选了「放弃改动，切过去」
    DropContinue,
    /// 用户在确认框里选了「算了」
    DropCancel,
}

/// 页面用假的 origin 承载（见 `with_url`），页面自身和页内锚点都落在这个 origin 下。
const APP_ORIGIN: &str = "http://mdpeek.localhost";

/// 只有指向页面本身的 URL 才留在预览器里滚动（含 `#锚点`、`/?query`）；
/// 其余一律外抛，mdpeek 只当浏览器，不自己承载网页。
fn is_self_navigation(url: &str) -> bool {
    let Some(rest) = url.strip_prefix(APP_ORIGIN) else {
        return false;
    };
    matches!(
        rest.split(['?', '#']).next().unwrap_or(""),
        "" | "/" | "/index.html"
    )
}

/// 相对链接在页面里会被解析成 `http://mdpeek.localhost/xxx`，
/// 这里按文档所在目录把它还原成磁盘路径（`dir` 就是那个目录，不是文件）。
fn resolve_local(url: &str, dir: Option<&Path>) -> Option<PathBuf> {
    let path = url.strip_prefix(APP_ORIGIN)?;
    let path = path.split(['?', '#']).next().unwrap_or("");
    Some(dir?.join(percent_decode(path.trim_start_matches('/'))))
}

/// 当前文档所在目录（绝对路径），相对图片按它解析；拖拽换文件时更新。
static DOC_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

/// Windows 的 `canonicalize()` 返回 `\\?\` 开头的长路径（UNC 则是 `\\?\UNC\`）。
/// 这个前缀不是给 URL 和显示用的——`file:///` + `\\?\F:\...` 拼出来的 base
/// 浏览器根本不认，相对图片会算歪，所以只留普通形式。
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p
    }
}

fn set_doc_dir(path: &Path) {
    // 只传了文件名（"x.md"）时 parent() 是空串，落回工作目录
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .and_then(|p| p.canonicalize().ok())
        .map(strip_verbatim)
        .or_else(|| std::env::current_dir().ok().map(strip_verbatim));
    *DOC_DIR.lock().unwrap() = base;
}

fn doc_dir() -> Option<PathBuf> {
    DOC_DIR.lock().unwrap().clone()
}

/// 只认图片扩展名：渲染出来的页面能执行 Markdown 里的原始 HTML，
/// 若什么类型都照单全收，就等于顺手给页面开了一个读任意本地文件的洞。
fn image_mime(path: &str) -> Option<&'static str> {
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("svg") => Some("image/svg+xml"),
        Some("bmp") => Some("image/bmp"),
        Some("avif") => Some("image/avif"),
        Some("ico") => Some("image/x-icon"),
        _ => None,
    }
}

/// 页面里的 `![](pic.png)` 会被解析成 `http://mdpeek.localhost/pic.png`，
/// 而这个协议原本只服务内嵌的 4 个资源，所以图片一律 404；这里补上读盘。
///
/// 两条来源：
/// - `?p=<绝对路径>`：页面用文档真实目录解析好之后回传的。`../pic.png`、
///   `C:\a\b.png`、`file:///C:/a/b.png` 都只能走这条——页面 origin 是假的根，
///   浏览器在发出请求前就把 `..` 规范化掉了，协议这边根本看不到原始写法。
/// - 路径本身：没被页面重写的相对路径，按当前文档目录兜底。
fn doc_asset(url_path: &str, query: Option<&str>) -> Option<(Vec<u8>, &'static str)> {
    let target = match query.and_then(|q| q.strip_prefix("p=")) {
        Some(abs) => percent_decode(abs),
        None => {
            let rel = percent_decode(url_path.trim_start_matches('/'));
            let rel = Path::new(&rel);
            // `../pic.png` 要放行（往上找图是常见写法），但带盘符或根目录的路径不收——
            // 那样 Path::join 会把文档目录整个丢掉
            if rel
                .components()
                .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            {
                return None;
            }
            doc_dir()?.join(rel).to_string_lossy().into_owned()
        }
    };
    let mime = image_mime(&target)?;
    fs::read(&target).ok().map(|b| (b, mime))
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// 只解 `%XX`；路径里的 `+` 是字面量，不按空格处理。
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // 全程按字节走：`%` 后面可能直接跟一个多字节字符，
        // 用 &s[i+1..i+3] 切会切在字符中间 panic
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(hi), Some(lo)) = (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// 交给系统 shell 打开：http/https 走默认浏览器，磁盘路径走文件关联。
// 有意不用 `cmd /c start`——URL 里的 `&` `%` `^` 会被 cmd.exe 二次解析；
// 也不用 `explorer.exe`——它会把 `/xxx` 形状的参数当自己的开关。
#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_cmd: i32,
    ) -> *mut std::ffi::c_void;
}

fn shell_open(target: &str) {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    let wide: Vec<u16> = OsStr::new(target)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // 返回码 <= 32 表示失败，这里没什么可做的：链接打不开不该拖累预览器
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        );
    }
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

/// 先写同目录的临时文件，再整体替换目标。
/// 直接 `fs::write` 是先截断再写，写到一半失败（磁盘满、进程被杀）原文件就废了。
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".mdpeek-{}.tmp", std::process::id()));
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// WebView2 默认把用户数据放在 exe 旁边（`mdpeek.exe.WebView2\`），
/// 装进 Program Files 这类只读目录就直接起不来，所以挪到 %LOCALAPPDATA%\mdpeek。
fn webview_data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("mdpeek");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn reg(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("reg.exe")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

fn register_app() {
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    let icon = format!("\"{}\",0", exe);
    let open = format!("\"{}\" \"%1\"", exe);
    let cmds: Vec<Vec<String>> = vec![
        vec!["add", r"HKCU\Software\Classes\mdpeek.md", "/ve", "/d", "Markdown Document", "/f"],
        vec!["add", r"HKCU\Software\Classes\mdpeek.md\DefaultIcon", "/ve", "/d", &icon, "/f"],
        vec!["add", r"HKCU\Software\Classes\mdpeek.md\shell\open\command", "/ve", "/d", &open, "/f"],
        vec!["add", r"HKCU\Software\Classes\.md\OpenWithProgids", "/v", "mdpeek.md", "/f"],
        vec!["add", r"HKCU\Software\Classes\.markdown\OpenWithProgids", "/v", "mdpeek.md", "/f"],
        vec!["add", r"HKCU\Software\Classes\Applications\mdpeek.exe\shell\open\command", "/ve", "/d", &open, "/f"],
        vec!["add", r"HKCU\Software\Classes\Applications\mdpeek.exe\SupportedTypes", "/v", ".md", "/f"],
        vec!["add", r"HKCU\Software\Classes\Applications\mdpeek.exe\SupportedTypes", "/v", ".markdown", "/f"],
        vec!["add", r"HKCU\Software\mdpeek\Capabilities", "/v", "ApplicationName", "/d", "mdpeek", "/f"],
        vec!["add", r"HKCU\Software\mdpeek\Capabilities", "/v", "ApplicationDescription", "/d", "Tiny Markdown viewer", "/f"],
        vec!["add", r"HKCU\Software\mdpeek\Capabilities\FileAssociations", "/v", ".md", "/d", "mdpeek.md", "/f"],
        vec!["add", r"HKCU\Software\mdpeek\Capabilities\FileAssociations", "/v", ".markdown", "/d", "mdpeek.md", "/f"],
        vec!["add", r"HKCU\Software\RegisteredApplications", "/v", "mdpeek", "/d", r"SOFTWARE\mdpeek\Capabilities", "/f"],
    ]
    .into_iter()
    .map(|v| v.into_iter().map(String::from).collect())
    .collect();
    for c in &cmds {
        let refs: Vec<&str> = c.iter().map(String::as_str).collect();
        if let Err(e) = reg(&refs) {
            eprintln!("reg {} failed: {}", c[1], e);
        }
    }
    println!("registered mdpeek for .md/.markdown (HKCU)");
}

fn unregister_app() {
    for key in [
        r"HKCU\Software\Classes\mdpeek.md",
        r"HKCU\Software\Classes\Applications\mdpeek.exe",
        r"HKCU\Software\mdpeek",
    ] {
        let _ = reg(&["delete", key, "/f"]);
    }
    for (key, val) in [
        (r"HKCU\Software\Classes\.md\OpenWithProgids", "mdpeek.md"),
        (r"HKCU\Software\Classes\.markdown\OpenWithProgids", "mdpeek.md"),
        (r"HKCU\Software\RegisteredApplications", "mdpeek"),
    ] {
        let _ = reg(&["delete", key, "/v", val, "/f"]);
    }
    println!("unregistered mdpeek");
}

fn load_and_render(webview: &WebView, window: &Window, path: &Path) {
    set_doc_dir(path);
    match fs::read(path) {
        Ok(bytes) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let raw_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let js_name = raw_name.replace('\\', "\\\\").replace('\'', "\\'");
            // 图片得按文档真实目录解析：页面 origin 是个假的根，浏览器会把 `..` 吃掉，
            // 也会把 `C:\...` 当成 c: 协议，所以页面拿到目录自己算
            let js_dir = doc_dir()
                .map(|p| p.to_string_lossy().replace('\\', "\\\\").replace('\'', "\\'"))
                .unwrap_or_default();
            let _ = webview.evaluate_script(&format!(
                "render('{}', '{}', '{}')",
                b64, js_name, js_dir
            ));
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

/// 窗口图标取自 exe 内嵌资源：build.rs 用 winres 把 assets/mdpeek.ico 写成资源 ID 1。
/// tao 不会自动继承 exe 图标——不显式设置的话，标题栏只能拿到 Windows 通用默认图标。
fn window_icon() -> Option<Icon> {
    use tao::platform::windows::IconExtWindows;
    Icon::from_resource(1, None).ok()
}

fn main() {
    let arg1 = std::env::args().nth(1);
    match arg1.as_deref() {
        Some("--register") => return register_app(),
        Some("--unregister") => return unregister_app(),
        _ => {}
    }
    let mut current_file = arg1.map(PathBuf::from);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();
    let refocus_proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("mdpeek")
        .with_inner_size(tao::dpi::LogicalSize::new(1000.0, 800.0))
        .with_window_icon(window_icon())
        .build(&event_loop)
        .expect("create window");

    let ipc_proxy = proxy.clone();
    let nav_proxy = proxy.clone();
    let new_window_proxy = proxy.clone();
    let mut web_context = WebContext::new(Some(webview_data_dir()));
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url("http://mdpeek.localhost/")
        .with_default_context_menus(false)
        // WebView2 默认自己吃掉 Ctrl+O/T/R/P 等浏览器加速键，快捷键永远到不了页面。
        // 关掉之后 Ctrl+ 组合键才归我们处理（Ctrl+C/V/X/A 属于编辑操作，不受影响）。
        .with_browser_accelerator_keys(false)
        // 普通单击（以及页内跳转的链接）都走这里：只有指向页面自身的导航放行，
        // 其余取消掉并外抛给系统，预览器自己绝不跳去别处。
        .with_navigation_handler(move |url| {
            if is_self_navigation(&url) {
                return true;
            }
            let _ = nav_proxy.send_event(UserEvent::OpenUrl(url));
            false
        })
        // Ctrl+单击 / 中键 / target="_blank" 在 WebView2 里是新窗口请求，不走上面的导航回调
        .with_new_window_req_handler(move |url, _features| {
            let _ = new_window_proxy.send_event(UserEvent::OpenUrl(url));
            NewWindowResponse::Deny
        })
        .with_custom_protocol("mdpeek".into(), move |_id, req: Request<Vec<u8>>| {
            let path = req.uri().path();
            // 先查内嵌资源，其余当文档目录下的相对路径读盘（图片）
            let (body, mime): (Cow<'static, [u8]>, &'static str) = match asset(path) {
                Some((body, mime)) => (Cow::Borrowed(body), mime),
                None => match doc_asset(path, req.uri().query()) {
                    Some((bytes, mime)) => (Cow::Owned(bytes), mime),
                    None => {
                        return Response::builder()
                            .status(404)
                            .body(Cow::Borrowed(&b""[..]))
                            .unwrap();
                    }
                },
            };
            Response::builder()
                .header("content-type", mime)
                .header("cache-control", "no-store")
                .body(body)
                .unwrap()
        })
        .with_ipc_handler(move |req| {
            let body = req.body();
            // 编辑器保存：正文以 "save:" 前缀整体带过来，不走命令名查表
            if let Some(text) = body.strip_prefix("save:") {
                let _ = ipc_proxy.send_event(UserEvent::Save(text.to_string()));
                return;
            }
            // 编辑器的脏状态同步过来，拖入新文件时要靠它决定能不能换保存目标
            if let Some(flag) = body.strip_prefix("dirty:") {
                let _ = ipc_proxy.send_event(UserEvent::EditorDirty(flag == "1"));
                return;
            }
            let ev = match body.as_str() {
                "ready" => Some(UserEvent::PageReady),
                "reload" => Some(UserEvent::Reload),
                "open-explorer" => Some(UserEvent::OpenExplorer),
                "copy-path" => Some(UserEvent::CopyPath),
                "copy-source" => Some(UserEvent::CopySource),
                "edit-notepad" => Some(UserEvent::EditNotepad),
                "toggle-ontop" => Some(UserEvent::ToggleOnTop),
                "drop-continue" => Some(UserEvent::DropContinue),
                "drop-cancel" => Some(UserEvent::DropCancel),
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
    // 启动兜底：从 Explorer 双击启动时，本进程可能抢不到前台，
    // 键盘事件就进不了 webview（鼠标右键不受影响，所以表现成"只有右键能用"）。
    // 只在整个生命周期里一次都没拿到过焦点时才补抢，避免把用户切走的窗口抢回来。
    let mut ever_focused = false;
    let mut refocus_armed = false;
    // 编辑器脏状态 + 等待用户确认的拖入文件。切文件前必须看这两个：
    // 页面会拒绝覆盖未保存的缓冲区，Rust 侧要是照切 current_file，
    // 接着按 Ctrl+S 就会把旧正文写进新文件。
    let mut editor_dirty = false;
    let mut pending_drop: Option<PathBuf> = None;
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
                    ever_focused = true;
                    let _ = webview.focus();
                }
                WindowEvent::DroppedFile(path) => {
                    if editor_dirty {
                        // 缓冲区还没保存：先问一句。决不能让 Rust 这边先把
                        // current_file 换掉，否则 Ctrl+S 会写错文件。
                        pending_drop = Some(path);
                        let _ = webview.evaluate_script("mdpeekConfirmDrop()");
                    } else {
                        current_file = Some(path.clone());
                        setup_watch(&path, &mut _watcher);
                        load_and_render(&webview, &window, &path);
                    }
                }
                _ => {}
            },
            Event::UserEvent(ev) => match ev {
                UserEvent::PageReady => {
                    window.set_focus();
                    let _ = webview.focus();
                    if !refocus_armed {
                        refocus_armed = true;
                        let p = refocus_proxy.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(350));
                            let _ = p.send_event(UserEvent::Refocus);
                        });
                    }
                    if let Some(f) = current_file.clone() {
                        load_and_render(&webview, &window, &f);
                    }
                }
                UserEvent::Refocus => {
                    if !ever_focused {
                        window.set_focus();
                    }
                    let _ = webview.focus();
                }
                UserEvent::EditorDirty(v) => editor_dirty = v,
                UserEvent::DropContinue => {
                    if let Some(path) = pending_drop.take() {
                        current_file = Some(path.clone());
                        setup_watch(&path, &mut _watcher);
                        load_and_render(&webview, &window, &path);
                    }
                }
                UserEvent::DropCancel => {
                    pending_drop = None;
                    // 确认框会抢走焦点，还回来
                    let _ = webview.focus();
                }
                UserEvent::ToggleOnTop => {
                    on_top = !on_top;
                    window.set_always_on_top(on_top);
                }
                UserEvent::Save(text) => {
                    if let Some(f) = current_file.clone() {
                        match write_atomic(&f, text.as_bytes()) {
                            Ok(()) => {
                                let _ = webview.evaluate_script("mdpeekSaved(true, '')");
                            }
                            Err(e) => {
                                let msg = e.to_string().replace('\\', "\\\\").replace('\'', "\\'");
                                let _ = webview
                                    .evaluate_script(&format!("mdpeekSaved(false, '{}')", msg));
                            }
                        }
                    }
                }
                UserEvent::OpenUrl(url) => {
                    // 相对链接还原成磁盘路径再交给系统；http(s) 等原样外抛
                    let target = resolve_local(&url, doc_dir().as_deref())
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or(url);
                    shell_open(&target);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_navigation_keeps_page_and_anchors_inside() {
        assert!(is_self_navigation("http://mdpeek.localhost/"));
        assert!(is_self_navigation("http://mdpeek.localhost"));
        assert!(is_self_navigation("http://mdpeek.localhost/index.html"));
        assert!(is_self_navigation("http://mdpeek.localhost/#h-3"));
        assert!(is_self_navigation("http://mdpeek.localhost/?a=1"));
    }

    #[test]
    fn external_links_are_handed_off() {
        assert!(!is_self_navigation("https://example.com/a#b"));
        assert!(!is_self_navigation("mailto:someone@example.com"));
        assert!(!is_self_navigation("http://mdpeek.localhost/other.md"));
        assert!(!is_self_navigation("http://evil.localhost/"));
    }

    #[test]
    fn relative_links_map_back_to_disk() {
        // 传进来的是文档所在目录
        let dir = Path::new(r"C:\docs");
        assert_eq!(
            resolve_local("http://mdpeek.localhost/other.md", Some(dir)),
            Some(PathBuf::from(r"C:\docs\other.md"))
        );
        assert_eq!(
            resolve_local("http://mdpeek.localhost/sub/a.md#x", Some(dir)),
            Some(PathBuf::from(r"C:\docs\sub\a.md"))
        );
        assert_eq!(resolve_local("https://example.com", Some(dir)), None);
        assert_eq!(resolve_local("http://mdpeek.localhost/a.md", None), None);
    }

    #[test]
    fn percent_decoding_handles_cjk_and_leaves_plus_alone() {
        assert_eq!(percent_decode("%E4%B8%AD%E6%96%87.md"), "中文.md");
        assert_eq!(percent_decode("a+b%20c.md"), "a+b c.md");
        // 坏转义按字面量保留，别把路径吃空
        assert_eq!(percent_decode("100%.md"), "100%.md");
        // `%` 后面直接跟多字节字符：按字节切才不会切在字符中间
        assert_eq!(percent_decode("中%中"), "中%中");
        assert_eq!(percent_decode("%E4%B8%AD%.png"), "中%.png");
    }

    #[test]
    fn only_images_are_served_from_disk() {
        // 非图片类型在读盘之前就被拒——这正是挡住"页面里存一段脚本读本地文件"的那道闸
        assert!(doc_asset("/secret.txt", None).is_none());
        assert!(doc_asset("/id_rsa", None).is_none());
        assert!(doc_asset("/notes.md", None).is_none());
        assert!(doc_asset("/", None).is_none());
        // ?p= 这条路上同样只认图片
        assert!(doc_asset("/__file", Some("p=C%3A%5CWindows%5Cwin.ini")).is_none());
    }

    #[test]
    fn rooted_relative_paths_cannot_escape_the_document_dir() {
        // 带盘符/根目录的相对路径会让 Path::join 丢掉文档目录，必须挡掉
        assert!(doc_asset("/C:/Windows/pic.png", None).is_none());
        assert!(doc_asset("//server/share/pic.png", None).is_none());
    }

    #[test]
    fn verbatim_prefix_is_stripped_before_going_to_the_page() {
        // 带 `\\?\` 的路径拼出来的 file:/// base 浏览器不认，相对图片会算歪
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\F:\projects\x")),
            PathBuf::from(r"F:\projects\x")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\x")),
            PathBuf::from(r"\\server\share\x")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"F:\projects\x")),
            PathBuf::from(r"F:\projects\x")
        );
    }

    #[test]
    fn images_resolve_against_the_document_dir() {
        let tmp = std::env::temp_dir().join("mdpeek-test-assets");
        let sub = tmp.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let png = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        fs::write(tmp.join("pic.png"), png).unwrap();
        fs::write(sub.join("inner.png"), png).unwrap();

        // 文档在 sub/ 下：同目录、以及 `../` 往上一级都要能读到
        set_doc_dir(&sub.join("doc.md"));
        assert_eq!(
            doc_asset("/inner.png", None),
            Some((png.to_vec(), "image/png"))
        );
        assert_eq!(
            doc_asset("/../pic.png", None).map(|(_, m)| m),
            Some("image/png")
        );
        assert!(doc_asset("/missing.png", None).is_none());

        // 页面重写后的绝对路径（`../`、`C:\`、`file:///` 最终都走这条）
        let abs = tmp.join("pic.png").to_string_lossy().into_owned();
        assert_eq!(
            doc_asset("/__file", Some(format!("p={}", abs).as_str())).map(|(_, m)| m),
            Some("image/png")
        );
        assert!(doc_asset("/__file", Some("p=")).is_none());

        let _ = fs::remove_dir_all(&tmp);
    }
}
