//! 真实转发引擎控制层。
//!
//! Rust 版不再是「只读预览」：它负责把 Python 生产引擎（`app/bridge_worker.py
//! --live`）真正拉起来、读它的结构化日志、优雅停掉，并在启动前清掉占着网关端口的
//! 残留 worker。采集 / OCR / 微信投递仍由久经生产验证的 Python 代码完成，Rust 侧
//! 只做进程管理与界面呈现——这样「能真正转发」不需要重写 OCR 与 UIA 自动化。
//!
//! 与 Python `app/gui.py` + `app/proc.py` 完全对齐的关键约定：
//! - 启动参数 `--live --config <cfg> --stop-file <file> --parent-pid <pid>`；
//! - 环境变量 `PYTHONUNBUFFERED / PYTHONIOENCODING / PYTHONUTF8 / PYTHONWARNINGS`；
//! - `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`，**任何子进程都不弹黑框**；
//! - 停止用哨兵文件（看门狗主线程抛 KeyboardInterrupt），失败才降级 kill；
//! - 启动前检查 8765 端口占用，只杀命令行里带 `bridge_worker.py` 的自己人。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

pub const GATEWAY_PORT: u16 = 8765;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const OFFICIAL_DIR: &str = "喵喵钉钉-微信消息转发桥";

#[cfg(windows)]
fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(windows)]
fn no_window_group(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
}

#[cfg(not(windows))]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

#[cfg(not(windows))]
fn no_window_group(cmd: &mut Command) -> &mut Command {
    cmd
}

// ────────────────────────────── 极简 JSON 字段提取 ──────────────────────────────
// worker 的日志是 Python ``json.dumps(..., ensure_ascii=False)`` 单行输出；
// 只需要 t / level / msg / kind / worker 这几个字符串字段，不值得为此引入 serde。

/// 解析一个 JSON 字符串字面量，返回 (值, 消耗到的字节下标)。
fn json_string(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                i += 1;
                if i >= b.len() {
                    return None;
                }
                match b[i] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000c}'),
                    b'u' => {
                        if i + 4 >= b.len() {
                            return None;
                        }
                        if let Ok(v) = u32::from_str_radix(&s[i + 1..i + 5], 16) {
                            if let Some(c) = char::from_u32(v) {
                                out.push(c);
                            }
                        }
                        i += 4;
                    }
                    c => out.push(c as char),
                }
                i += 1;
            }
            _ => {
                let ch = s[i..].chars().next()?;
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    None
}

/// 从扁平 JSON 对象里取一个字符串字段（键均为 ASCII，无嵌套同名歧义）。
pub fn json_field(obj: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = obj.find(&needle)?;
    let rest = &obj[pos + needle.len()..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    json_string(rest).map(|(v, _)| v)
}

// ────────────────────────────── 后端发现 ──────────────────────────────

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
}

fn appdata_root() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join(OFFICIAL_DIR)
}

/// 一次解析结果：Rust 界面据此组装 worker 命令行。
#[derive(Clone, Debug)]
pub struct Backend {
    /// 含 `app/`、`bridge/`、`run.py` 的后端根目录。
    pub root: PathBuf,
    /// 带重依赖的工作解释器（`runtime/python.exe`）。
    pub python: PathBuf,
    pub worker_script: PathBuf,
    pub config: PathBuf,
    pub stop_file: PathBuf,
    /// 展示用来源说明。
    pub source: String,
    /// 是否为安装包自带的引擎（决定配置位置与界面文案）。
    pub bundled: bool,
}

impl Backend {
    pub fn short(&self) -> String {
        format!(
            "{} · {}",
            self.source,
            self.root.file_name().map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
        )
    }
}

fn root_candidates(db_path: Option<&str>) -> Vec<(PathBuf, String)> {
    let mut v: Vec<(PathBuf, String)> = Vec::new();
    if let Some(p) = env_path("MIAOMIAO_BRIDGE_HOME") {
        v.push((p, "环境变量指定".into()));
    }

    // 0.3.0 独立版：安装目录里自带的 backend/ 必须优先于机器上任何
    // 已经装好的 Python 正式版，否则「发给别人也能用」就不成立。
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("backend");
            if bundled.join("app").join("bridge_worker.py").is_file()
                && bundled.join("run.py").is_file()
            {
                // 测试用：强制只用自带引擎，模拟一台没装正式版的干净机器。
                if std::env::var_os("MIAOMIAO_FORCE_BUNDLED").is_some() {
                    return vec![(bundled, "内置转发引擎".into())];
                }
                v.push((bundled, "内置转发引擎".into()));
            }
        }
    }

    if let Some(db) = db_path {
        if let Some(root) = PathBuf::from(db).parent().and_then(|d| d.parent()) {
            v.push((root.to_path_buf(), "数据库所在目录".into()));
        }
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(key) {
            v.push((Path::new(&base).join(OFFICIAL_DIR), "正式版安装目录".into()));
        }
    }
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        v.push((
            Path::new(&base).join("Programs").join(OFFICIAL_DIR),
            "用户安装目录".into(),
        ));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push((dir.to_path_buf(), "程序目录".into()));
            let mut cur = dir.to_path_buf();
            for _ in 0..4 {
                if let Some(p) = cur.parent() {
                    cur = p.to_path_buf();
                    v.push((cur.clone(), "上级工作区".into()));
                }
            }
        }
    }
    v.push((
        PathBuf::from(r"C:\Program Files\喵喵钉钉-微信消息转发桥"),
        "默认安装目录".into(),
    ));
    v
}

/// 自带后端目录的特征：`.../安装目录/backend/runtime/python.exe`。
fn is_bundled(root: &Path) -> bool {
    root.file_name()
        .map(|n| n.eq_ignore_ascii_case("backend"))
        .unwrap_or(false)
        && root.join("runtime").join("python.exe").is_file()
}

fn find_python(root: &Path, bundled: bool) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = Vec::new();
    // 自带引擎必须用它自己的解释器。绝不能让 %APPDATA% 里残留的
    // `_env.json` 缓存把 worker 指回机器上另一套 Python（那等于没独立）。
    if bundled {
        c.push(root.join("runtime").join("python.exe"));
    }
    for k in ["MIAOMIAO_WORKER_PYTHON", "DDWX_WORKER_PYTHON"] {
        if let Some(p) = env_path(k) {
            c.push(p);
        }
    }
    // Python GUI 的环境探测缓存（安装到 Program Files 时落在 %APPDATA%）。
    if !bundled {
        for f in [root.join("app").join("_env.json"), appdata_root().join("_env.json")] {
            if let Ok(txt) = fs::read_to_string(&f) {
                if let Some(w) = json_field(&txt, "worker") {
                    if !w.trim().is_empty() {
                        c.push(PathBuf::from(w));
                    }
                }
            }
        }
    }
    for rel in [
        "runtime/python.exe",
        "runtime/Scripts/python.exe",
        ".venv/Scripts/python.exe",
        "venv/Scripts/python.exe",
        "python/python.exe",
    ] {
        c.push(root.join(rel));
    }
    let me = std::env::current_exe().ok();
    c.into_iter().find(|p| {
        p.is_file()
            && p.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false)
            && me.as_ref() != Some(p)
    })
}

fn find_config(root: &Path, bundled: bool) -> Option<PathBuf> {
    for k in ["MIAOMIAO_BRIDGE_CONFIG", "DDWX_CONFIG"] {
        if let Some(p) = env_path(k) {
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // 独立版把 config.yaml 放在安装根（backend/ 的上一级）：
    // `Config.resolve("data/bridge.db")` 是相对配置文件目录解析的，
    // 这样 worker 写库和界面读库天然指向同一个 `<安装目录>/data/bridge.db`。
    if bundled {
        if let Some(parent) = root.parent() {
            let c = parent.join("config.yaml");
            if c.is_file() {
                return Some(c);
            }
        }
    }
    // 与 Python `paths.config_path()` 同口径：程序目录不可写时配置在 %APPDATA%。
    let user = appdata_root().join("config.yaml");
    if user.is_file() {
        return Some(user);
    }
    let local = root.join("config.yaml");
    local.is_file().then_some(local)
}

pub fn discover(db_path: Option<&str>) -> Result<Backend, String> {
    let chosen = root_candidates(db_path).into_iter().find(|(r, _)| {
        r.join("app").join("bridge_worker.py").is_file() && r.join("run.py").is_file()
    });
    let (root, source) = chosen.ok_or_else(|| {
        "未找到转发引擎（需要 app/bridge_worker.py + run.py）。\n\
         独立版应自带 backend/ 目录；若确实丢失，请重新运行安装包，\n\
         或用环境变量 MIAOMIAO_BRIDGE_HOME 指向 Python 正式版安装目录。"
            .to_owned()
    })?;
    let bundled = is_bundled(&root);

    let python = find_python(&root, bundled).ok_or_else(|| {
        format!(
            "后端目录 {} 里没有可用的工作解释器 runtime/python.exe。\n\
             可用 MIAOMIAO_WORKER_PYTHON 指定一个装了 pyyaml / pillow / numpy / rapidocr 的 python.exe。",
            root.display()
        )
    })?;
    let config = find_config(&root, bundled).ok_or_else(|| {
        format!("未找到 config.yaml（安装目录 / 后端目录 {} / %APPDATA%）。", root.display())
    })?;

    // 哨兵文件与 bridge.db 同目录；data_root() 与 datasource::load() 共用，
    // 独立版下就是 `<安装目录>/data`。
    let stop_file = crate::datasource::data_root().join("run.stop");
    if let Some(d) = stop_file.parent() {
        let _ = fs::create_dir_all(d);
    }

    Ok(Backend {
        worker_script: root.join("app").join("bridge_worker.py"),
        root,
        python,
        config,
        stop_file,
        source,
        bundled,
    })
}

// ────────────────────────────── 端口残留清理 ──────────────────────────────

fn port_owner_pids(port: u16) -> Vec<u32> {
    let mut cmd = Command::new("netstat");
    cmd.args(["-ano", "-p", "TCP"]);
    let out = match no_window(&mut cmd).output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let needle = format!(":{port}");
    let mut pids: Vec<u32> = Vec::new();
    for line in text.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() < 5 || !p[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !p[3].eq_ignore_ascii_case("LISTENING") || !p[1].ends_with(&needle) {
            continue;
        }
        if let Ok(pid) = p[4].parse::<u32>() {
            if pid > 0 && !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    pids
}

/// 供无人值守自检判断网关是否已经监听。
pub fn port_busy(port: u16) -> bool {
    !port_owner_pids(port).is_empty()
}

fn proc_cmdline(pid: u32) -> String {
    let mut cmd = Command::new("powershell.exe");
    let ps = format!(
        "(Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\").CommandLine"
    );
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps]);
    match no_window(&mut cmd).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_owned(),
        Err(_) => String::new(),
    }
}

fn kill_tree(pid: u32) {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/F", "/T"]);
    let _ = no_window(&mut cmd).spawn();
}

/// 与 Python `_reap_stale_worker()` 同策略：只杀确认是自己的 worker。
/// 返回 Ok 里的字符串是给用户看的一行提示（空串表示端口本来就空闲）。
pub fn reap_stale(port: u16, official_gui_open: bool) -> Result<String, String> {
    let pids = port_owner_pids(port);
    if pids.is_empty() {
        return Ok(String::new());
    }
    if official_gui_open {
        return Err(format!(
            "Python 正式版界面正在运行，且网关端口 {port} 已被占用。\n\
             请先在正式版窗口按 F6 停止转发，再回到 Rust 版启动，避免两个引擎同时投递。"
        ));
    }
    for &pid in &pids {
        let cmd = proc_cmdline(pid);
        if !cmd.contains("bridge_worker.py") {
            let brief: String = cmd.chars().take(160).collect();
            return Err(format!(
                "端口 {port} 被其它程序占用（PID {pid}）：\n{}\n请先关闭该程序再启动。",
                if brief.is_empty() { "取不到命令行" } else { &brief }
            ));
        }
    }
    for &pid in &pids {
        kill_tree(pid);
    }
    for _ in 0..15 {
        if port_owner_pids(port).is_empty() {
            return Ok(format!("已清理 {} 个残留转发进程", pids.len()));
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "端口 {port} 释放不掉，残留进程 {pids:?} 请在任务管理器里手动结束。"
    ))
}

// ────────────────────────────── worker 进程 ──────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Live,
    Check,
}

impl Mode {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Live => "--live",
            Self::Check => "--check",
        }
    }
}

#[derive(Clone, Debug)]
pub enum WorkerEvent {
    Log {
        time: String,
        level: String,
        text: String,
    },
    Control {
        kind: String,
        detail: String,
    },
    Finished {
        code: i32,
        graceful: bool,
    },
}

pub struct Worker {
    pub mode: Mode,
    pub pid: u32,
    pub started_at: Instant,
    child: Child,
    rx: Receiver<WorkerEvent>,
    stop_file: PathBuf,
    stopping_at: Option<Instant>,
    killed: bool,
    finished: bool,
}

impl Worker {
    pub fn is_stopping(&self) -> bool {
        self.stopping_at.is_some()
    }

    /// 写停止哨兵文件；看门狗会让 worker 走 KeyboardInterrupt 优雅收尾。
    /// 4 秒后仍未退出，`poll()` 会自动升级到 kill + taskkill /T。
    pub fn request_stop(&mut self) {
        if self.finished || self.stopping_at.is_some() {
            return;
        }
        self.stopping_at = Some(Instant::now());
        let _ = fs::write(&self.stop_file, "stop");
    }

    /// 非阻塞地收取日志、检测退出、必要时强杀。每帧调用一次。
    pub fn poll(&mut self) -> Vec<WorkerEvent> {
        let mut ev = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            ev.push(e);
        }
        if !self.finished {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.finished = true;
                ev.push(WorkerEvent::Finished {
                    code: status.code().unwrap_or(-1),
                    graceful: self.stopping_at.is_some() && !self.killed,
                });
            }
        }
        if let Some(t) = self.stopping_at {
            if !self.finished && !self.killed && t.elapsed() >= Duration::from_secs(4) {
                self.killed = true;
                let _ = self.child.kill();
                kill_tree(self.pid);
            }
        }
        ev
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !self.finished {
            // 界面被关掉时也不留孤儿进程；worker 自身还有 --parent-pid 看门狗兜底。
            let _ = fs::write(&self.stop_file, "stop");
            let _ = self.child.kill();
        }
    }
}

fn classify_line(line: &str, is_stderr: bool, tx: &Sender<WorkerEvent>) {
    let s = line.trim();
    if s.is_empty() {
        return;
    }
    if let Some(json) = s.strip_prefix("@@LOG@@") {
        let _ = tx.send(WorkerEvent::Log {
            time: json_field(json, "t").unwrap_or_else(crate::datasource::local_clock),
            level: json_field(json, "level").unwrap_or_else(|| "INFO".into()),
            text: json_field(json, "msg").unwrap_or_default(),
        });
        return;
    }
    if let Some(json) = s.strip_prefix("@@WORKER@@") {
        let kind = json_field(json, "kind").unwrap_or_default();
        let detail = json_field(json, "traceback").unwrap_or_default();
        let _ = tx.send(WorkerEvent::Control { kind, detail });
        return;
    }
    let _ = tx.send(WorkerEvent::Log {
        time: crate::datasource::local_clock(),
        level: if is_stderr { "WARN" } else { "INFO" }.into(),
        text: s.to_owned(),
    });
}

/// 按行读 stdout/stderr；用 from_utf8_lossy 容忍偶发的非 UTF-8 字节。
fn pump<R: Read + Send + 'static>(mut r: R, tx: Sender<WorkerEvent>, is_stderr: bool) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => {
                if !buf.is_empty() {
                    let s = String::from_utf8_lossy(&buf);
                    classify_line(&s, is_stderr, &tx);
                }
                break;
            }
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    let part: Vec<u8> = buf.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&part)
                        .trim_end_matches(['\r', '\n'])
                        .to_owned();
                    classify_line(&line, is_stderr, &tx);
                }
            }
            Err(_) => break,
        }
    }
}

/// 启动真正的转发（或自检）进程。
pub fn spawn(backend: &Backend, mode: Mode, official_gui_open: bool) -> Result<Worker, String> {
    if mode == Mode::Live {
        // 端口被残留 worker 占着时新进程会 WinError 10048 直接退出，先清场。
        reap_stale(GATEWAY_PORT, official_gui_open)?;
    }
    let _ = fs::remove_file(&backend.stop_file);

    let mut cmd = Command::new(&backend.python);
    cmd.arg(&backend.worker_script)
        .arg(mode.flag())
        .arg("--config")
        .arg(&backend.config)
        .arg("--stop-file")
        .arg(&backend.stop_file)
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .current_dir(&backend.root)
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("PYTHONWARNINGS", "ignore")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = no_window_group(&mut cmd).spawn().map_err(|e| {
        format!(
            "启动工作进程失败：{e}\n解释器：{}\n后端：{}",
            backend.python.display(),
            backend.root.display()
        )
    })?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel::<WorkerEvent>();
    if let Some(out) = child.stdout.take() {
        let t = tx.clone();
        thread::spawn(move || pump(out, t, false));
    }
    if let Some(err) = child.stderr.take() {
        let t = tx.clone();
        thread::spawn(move || pump(err, t, true));
    }
    drop(tx);

    Ok(Worker {
        mode,
        pid,
        started_at: Instant::now(),
        child,
        rx,
        stop_file: backend.stop_file.clone(),
        stopping_at: None,
        killed: false,
        finished: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_field_reads_worker_log() {
        let s = r#"{"t": "17:03:12", "level": "WARNING", "name": "bridge", "msg": "钉钉 → 微信：ok"}"#;
        assert_eq!(json_field(s, "t").as_deref(), Some("17:03:12"));
        assert_eq!(json_field(s, "level").as_deref(), Some("WARNING"));
        assert_eq!(json_field(s, "msg").as_deref(), Some("钉钉 → 微信：ok"));
    }

    #[test]
    fn json_string_handles_escapes() {
        let s = r#"{"msg": "带\"引号\"、反斜杠\\和换行\n结束"}"#;
        assert_eq!(
            json_field(s, "msg").as_deref(),
            Some("带\"引号\"、反斜杠\\和换行\n结束")
        );
    }

    #[test]
    fn json_field_reads_env_cache() {
        let s = r#"{"gui": "", "worker": "C:\\Program Files\\x\\runtime\\python.exe"}"#;
        assert_eq!(
            json_field(s, "worker").as_deref(),
            Some(r"C:\Program Files\x\runtime\python.exe")
        );
    }

    #[test]
    fn mode_flags_match_python_cli() {
        assert_eq!(Mode::Live.flag(), "--live");
        assert_eq!(Mode::Check.flag(), "--check");
    }

    #[test]
    fn discover_finds_installed_backend_on_this_machine() {
        // 开发机上装了 0.63 正式版；CI 上没有时允许「找不到」，但不能 panic。
        let r = discover(None);
        if let Ok(b) = r {
            assert!(b.worker_script.is_file());
            assert!(b.python.is_file());
            assert!(b.config.is_file());
        }
    }
}
