# mdpeek

一个轻量的 Windows Markdown 阅读器——外表像记事本一样简单，里面是全量渲染。

[English README](README.md)

![浅色](docs/screenshots/light-main.png)
![深色](docs/screenshots/dark-main.png)

## 功能

- GFM 全语法：标题、表格、任务列表、删除线、链接、图片
- 链接不会顶掉预览：点击（或 `Ctrl + 单击` / 中键）一律交给系统打开，`http(s)` 走默认浏览器、本地文件走文件关联，窗口自己绝不跳走；页内 `#锚点` 仍原地滚动
- 本地图片直接显示：相对路径、子目录、`../`、Windows 绝对路径、`file:///` 都按文档所在目录解析
- ` ```mermaid ` 代码块直接渲染成流程图、时序图等图表
- highlight.js 代码高亮
- 明/暗主题跟随系统，一键切换
- 文件监听实时刷新——磁盘上文件一变，预览自动更新（配合记事本用，就是零成本的"编辑 + 预览"组合）
- 大纲侧栏，滚动跟随高亮
- 源码视图：一键查看原始 Markdown
- 内置编辑器：直接在窗口里改 Markdown，`Ctrl + S` 保存，不用再绕去记事本
- 标题栏下方工具栏：编辑 / 源码 / 大纲 / 置顶 / 主题 / 缩放 / 打印，一点即达
- 窗口置顶、缩放、打印/导出 PDF、自定义右键菜单
- 单文件约 5 MB，免安装、非 Electron——Rust + 系统 WebView2

## 用法

```
mdpeek.exe 路径\文件.md
```

或者直接把 `.md` 文件拖进窗口。

## 快捷键与菜单

所有功能在工具栏和右键菜单里也都有。

| 按键 | 作用 |
|---|---|
| `Ctrl + E` | 切换编辑器 |
| `Ctrl + S` | 保存（编辑中） |
| `Ctrl + O` | 开/关大纲侧栏 |
| `Ctrl + M` | 切换 Markdown 源码视图 |
| `Ctrl + B` | 在资源管理器中显示文件 |
| `Ctrl + R` | 重新加载文件 |
| `Ctrl + T` | 切换明/暗主题 |
| `Ctrl + P` | 打印 / 导出 PDF |
| `Ctrl + 0` / `Ctrl + =` / `Ctrl + -` | 重置 / 放大 / 缩小 |
| `Ctrl + 滚轮` | 缩放 |
| `Esc` | 退出编辑器、关闭菜单或收起大纲 |

用记事本打开仍保留在右键菜单里。

## 设置默认程序

```
mdpeek.exe --register     :: 把 mdpeek 设为 .md / .markdown 的默认打开方式（当前用户，免管理员）
mdpeek.exe --unregister   :: 撤销
```

注册写入的是 exe 自身的绝对路径，所以请先把它放到你打算长期保留的位置再执行。

## 构建

```
cargo build --release
```

产物：`target\release\mdpeek.exe`，单文件免安装。需要 WebView2 运行时（Windows 10/11 自带）。

## 技术栈

Rust · [wry](https://github.com/tauri-apps/wry) + [tao](https://github.com/tauri-apps/tao) · WebView2 · marked · mermaid · highlight.js

## 协议

MIT —— 见 [LICENSE](LICENSE)。
