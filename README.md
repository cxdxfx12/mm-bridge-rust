# 喵喵钉钉-微信消息转发桥 · Rust 版

> 版本 **0.4.0** ｜ Windows x64 ｜ egui 0.37 ｜ 单文件独立安装包

把钉钉指定群的消息按规则转发到指定微信群。本仓库是这套系统的 **Rust 控制台 + 独立版安装器**；Python 正式版（0.63 及以前）在另一个仓库 `mm-dd-wx`。

**下载**：[Releases · v0.4.0](https://github.com/cxdxfx12/mm-bridge-rust/releases/tag/v0.4.0) → `MiaoMiaoBridge-Rust-Standalone-Setup-v0.4.0.exe`（单文件，约 157 MiB）。

**0.4.0 的关键变化**：左侧 5 个导航不再是只有「运行总览」有内容，另外 4 个管理页全部接上真实能力 —— 消息流水（搜索 / 筛选 / 详情 / CSV）、规则与会话（读改 `config.yaml` 并保留注释）、漏转对账（桥内漏投清单 + 导出）、设置中心（原文编辑、路径、数据库清理与 VACUUM）。所有查询仍只读 `bridge.db`；写操作只作用于独立版自己的配置与数据库。

**0.3.0 的关键变化**：安装包自带完整转发引擎。目标机器不需要装 Python、也不需要预装 0.63 正式版，双击一个 `Setup.exe` 就能真正开始转发。

---

## 架构

Rust 不重写钉钉 WAL 解密、OCR 和微信 UIA —— 那套生产逻辑久经实机验证，重写成本高、风险大。Rust 做的是**真正驱动它的控制台**：

| 层 | crate / 目录 | 职责 |
|---|---|---|
| 界面与控制 | `crates/bridge-gui` | egui 自绘界面；`runner.rs` 启停 worker、管道收 `@@LOG@@` 结构化日志、哨兵文件优雅停止；`datasource.rs` 只读 `bridge.db`；`startup.rs` 处理无黑窗、单实例、崩溃日志 |
| 纯逻辑 | `crates/bridge-core` | 消息指纹 `md5(norm_sender\|content)`（与 Python 版 golden 对拍）、只读快照统计 |
| 安装器 | `crates/bridge-setup` | 单 exe 自解压：释放 GUI + `backend/`，写快捷方式与卸载入口，注入网关 token |
| 转发引擎 | 安装包内 `backend/` | 嵌入式 Python 3.12 runtime + `app/` `bridge/` `run.py` `wxauto-main/` `tools/` `android/` + pyyaml / Pillow / numpy / onnxruntime / rapidocr / opencv |
| 视觉稿 | `design/ui-mockup.html` | 零外链浅色主题，`#2F6BED` 主色、13 类组件、6 种交互态；`theme.rs` / `widgets.rs` 是它的落地 |

采集、OCR、微信投递由 Python 引擎执行；Rust 通过 `runtime/python.exe app/bridge_worker.py --live` 拉起它，参数、环境变量、工作目录与 Python GUI 完全对齐。

### 为什么用 PE overlay

引擎归档约 147 MB / 4640 个文件。用 `include_bytes!` 会让 rustc 生成超大常量、`link.exe` 吃掉数 GB 内存；用 Win32 `RCDATA` 处理超大资源也不可靠。所以把归档直接 **append 在 Setup.exe 尾部**（Windows 加载器忽略最后一个节之后的数据），运行时从自身读出、CRC32 校验后解压 —— 编译期零负担，安装包仍是单文件。

---

## 目录结构

```
.
├── Cargo.toml               # workspace：core / gui / setup，release 体积优化
├── Cargo.lock
├── .cargo/config.toml       # rsproxy 镜像（国内构建更快）
├── crates/
│   ├── bridge-core/src/     # fingerprint.rs、data.rs（只读统计 / 流水 / 漏投 / CSV）、lib.rs
│   ├── bridge-gui/src/      # main.rs、pages.rs（4 个管理页）、config.rs、dbclean.rs、
│   │                        # runner.rs、datasource.rs、theme.rs、widgets.rs、startup.rs
│   └── bridge-setup/src/    # main.rs、bundle.rs（raw deflate + CRC32）
├── assets/
│   ├── app.ico / app_*.png / app_32.rgba   # 图标资源（构建必需，已入库）
│   ├── make_icon.py          # 生成多尺寸图标 + 运行时 RGBA
│   ├── embed_resources.rs    # build.rs 共用：rc.exe 嵌 ICON + VS_VERSIONINFO
│   └── pack_backend.py       # 打包 Python 引擎并追加到 Setup.exe
├── design/ui-mockup.html    # UI 视觉稿
└── README.md
```

---

## 从源码构建

**前提**（目前只支持 Windows）：

- Rust **1.85+**、MSVC Build Tools（VCTools `link.exe`）
- Windows SDK `rc.exe`（嵌图标与版本资源；缺失时只警告，仍能编译出无图标 exe）
- Python 3 + Pillow（仅生成图标时需要）
- 一份 Python 正式版安装目录，作为引擎归档源（可用 `MIAOMIAO_BACKEND_SRC` 或 `--src` 指向任意副本）

```bash
# 1. 测试（37 条：指纹 golden、只读数据层、配置读写、清理口径、归档路径穿越、token 注入等）
cargo test --workspace

# 2. 图标（如已提交的资源未改动可跳过）
python assets/make_icon.py

# 3. 先编 GUI，再编 Setup —— 顺序不能反，Setup 会嵌入 release 主程序
cargo build --release -p bridge-gui
cargo build --release -p bridge-setup

# 4. 打引擎归档并追加到 exe，产出 dist/*-Setup.exe
python assets/pack_backend.py --all --src "C:\Program Files\喵喵钉钉-微信消息转发桥"
```

只改了安装器代码时，归档不用重打，跑 `python assets/pack_backend.py --append` 即可。

如果**只想做「依赖正式版」的轻量包**（约 9 MB），跳过第 4 步；此时 GUI 会按「自带 backend → `MIAOMIAO_BRIDGE_HOME` → 正式版安装目录」的顺序找引擎。

### 仓库里为什么没有 exe 和 `backend.mmb1`

| 文件 | 体积 | 不入库原因 |
|---|---|---|
| `assets/backend.mmb1` | ~154 MB | GitHub 单文件硬限 100 MB；且由本机正式版目录生成，可能含本机凭证 |
| `dist/*-Setup.exe` | ~164 MB | 同上，构建产物 |
| `target/` | ~4 GB | Cargo 编译产物 |

三者都能用上面的命令重新生成。打包脚本还会主动**排除** `data/`（生产数据库）、`app/_env.json`（本机绝对路径缓存）、`.pyc`、正式版 GUI/卸载器，并把 `config.yaml` 里的钉钉凭证与 webhook 清空、`wxauto_path` 相对化。

---

## 使用

双击 Setup 安装后（默认 `%LOCALAPPDATA%\Programs\MiaoMiaoBridgeRustPreview`）：

| 操作 | 效果 |
|---|---|
| 「开始转发」或 **F5** | 拉起 worker，网关 `:8765` 监听，引擎日志实时回流界面 |
| **F6** | 哨兵文件优雅停止，端口释放，无孤儿进程 |
| **Ctrl+R** | 刷新只读数据面板 |
| 「打开配置」 | 编辑 `config.yaml` |
| `bridge-gui.exe --worker-smoke` | 无人值守自检：启动引擎 → 检查网关 → 收日志 → 优雅停止，结果写 `startup.log` |

### 五个页面（0.4.0 起全部可用）

| 导航 | 能力 |
|---|---|
| 运行总览 | KPI、实时日志、最近流水、F5/F6 真实启停 |
| 消息流水 | 最多 200 条真实入库记录，关键词搜索 + 状态筛选，点行看全文，导出 UTF-8 BOM CSV |
| 规则与会话 | 展示当前生效配置与 `routes` 规则；表单改采集 / 投递常用字段，写回 `config.yaml` 时保留注释并留 `.bak` |
| 漏转对账 | 基于 `bridge.db` 列出「从未成功投递」的消息（待投递 / 仅失败），可导出清单、启动自检、跳转正式版跨库对账 |
| 设置中心 | 运行环境路径、配置原文编辑、消息数据库体检 → 二次确认 → 删行 + `VACUUM`、关于与快捷键 |

> 清理按钮有硬闸门：只有**独立版自带数据库**且**引擎已停止**时才可用；接 Python 正式版库时永远是只读，避免误删生产数据。真正的「钉钉本地库 vs 入库」跨库对账仍由 Python 引擎的 `reconcile` 模块完成。

安装器参数：`--silent`（无弹窗）、`--no-shortcut`、`--dir <路径>`、`--uninstall`、`--purge`（卸载时连数据一起删，默认**保留** `data/`）。

**数据隔离**：配置与数据库只在 `<安装目录>/config.yaml` 和 `<安装目录>/data/bridge.db`，不读写正式版任何文件。首次安装会自动配好网关 token —— 本机装过正式版就继承旧值（手机端无需改动），干净机器随机生成 48 位十六进制值。

### 环境变量

| 变量 | 用途 |
|---|---|
| `MIAOMIAO_BACKEND_SRC` | 打包时指定 Python 引擎源目录 |
| `MIAOMIAO_FORCE_BUNDLED=1` | 强制只用自带引擎（模拟干净机器做验证） |
| `MIAOMIAO_FRESH_TOKEN=1` | 安装时不继承旧 token，强制随机生成 |
| `MIAOMIAO_BRIDGE_HOME` | 指回外部 Python 正式版后端 |
| `MIAOMIAO_BRIDGE_DB` | 指定只读数据源 |
| `MIAOMIAO_WORKER_PYTHON` / `DDWX_WORKER_PYTHON` | 指定工作解释器 |
| `MIAOMIAO_BRIDGE_CONFIG` / `DDWX_CONFIG` | 指定配置文件 |
| `MIAOMIAO_ALLOW_MULTI=1` | 跳过单实例检查，用于新旧版本并列验证（日常不用） |

---

## 已知约束

1. 当前配置为 `dingtalk.mode: "android"`（手机端无障碍采集），对方仍需装手机端采集 App，并填写电脑网关地址 + `config.yaml` 里的 token。这是既有采集方案的约束，不是独立版缺东西。
2. 模板清空了钉钉凭证与 webhook；若切换到 OCR / stream 模式，需自行在配置里补齐。
3. 只支持 Windows x64：依赖 Win32 DPI/窗口 API、`rc.exe` 资源编译、微信 UIA 投递。
