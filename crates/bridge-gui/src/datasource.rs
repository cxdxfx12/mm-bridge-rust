//! 适配层：定位 Python 正式版 `bridge.db`，只读取数，并检测正式版窗口。
//!
//! 方案 A 的边界：**只看、不写、不控制**。
//! - 连接一律 `SQLITE_OPEN_READ_ONLY`，再补 `PRAGMA query_only=ON` 双保险；
//! - 不启动 / 停止 worker，不碰 config.yaml，不改任何生产数据；
//! - 「打开正式版」按钮只是用 ShellExecute 打开正式版 GUI，由用户在正式版里按 F5。

use bridge_core::data::{self, Snapshot};
use rusqlite::{Connection, OpenFlags};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;

const OFFICIAL_TITLE: &str = "喵喵钉钉-微信消息转发桥";
const OFFICIAL_EXE: &str = "喵喵钉钉-微信消息转发桥.exe";
const OFFICIAL_DIR: &str = "喵喵钉钉-微信消息转发桥";

#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(time: *mut SystemTime);
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, name: *const u16) -> *mut c_void;
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *const c_void,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> isize;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn local_time() -> SystemTime {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    t
}

pub fn local_date() -> String {
    let t = local_time();
    format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
}

pub fn local_clock() -> String {
    let t = local_time();
    format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
}

/// Python 正式版 GUI 是否开着（Rust 版标题带「· Rust 预览版」，不会被误命中）。
fn official_gui_open() -> bool {
    let title = wide(OFFICIAL_TITLE);
    unsafe { !FindWindowW(ptr::null(), title.as_ptr()).is_null() }
}

/// 独立版自带后端时，数据一律落在安装目录自己的 `data/` 下，
/// 与 worker 的 `--config <安装目录>/config.yaml`（db_path 相对配置文件）严格一致。
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|p| p.to_path_buf())
}

pub fn has_bundled_backend() -> bool {
    exe_dir()
        .map(|d| {
            d.join("backend").join("runtime").join("python.exe").is_file()
                && d.join("backend").join("app").join("bridge_worker.py").is_file()
        })
        .unwrap_or(false)
}

fn bundled_db() -> Option<PathBuf> {
    if !has_bundled_backend() {
        return None;
    }
    exe_dir().map(|d| d.join("data").join("bridge.db"))
}

/// worker 与界面共用的数据目录（哨兵文件 / 数据库都在这里）。
pub fn data_root() -> PathBuf {
    if let Ok(p) = std::env::var("MIAOMIAO_BRIDGE_DB") {
        if let Some(d) = PathBuf::from(p).parent() {
            return d.to_path_buf();
        }
    }
    if let Some(d) = bundled_db() {
        return d.parent().unwrap().to_path_buf();
    }
    if let Some(db) = resolve_db() {
        if let Some(d) = db.parent() {
            return d.to_path_buf();
        }
    }
    exe_dir()
        .map(|d| d.join("data"))
        .unwrap_or_else(|| PathBuf::from("data"))
}

/// 按优先级找正式版数据库，发给别人用时可用环境变量覆盖。
pub fn candidate_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("MIAOMIAO_BRIDGE_DB") {
        v.push(PathBuf::from(p));
    }
    if let Some(d) = exe_dir() {
        v.push(d.join("data").join("bridge.db"));
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(key) {
            v.push(
                Path::new(&base)
                    .join(OFFICIAL_DIR)
                    .join("data")
                    .join("bridge.db"),
            );
        }
    }
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        v.push(
            Path::new(&base)
                .join("Programs")
                .join(OFFICIAL_DIR)
                .join("data")
                .join("bridge.db"),
        );
    }
    v.push(PathBuf::from(
        r"C:\Program Files\喵喵钉钉-微信消息转发桥\data\bridge.db",
    ));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("data").join("bridge.db"));
            // rust/target/release/bridge-gui.exe → 工作区根/data/bridge.db
            if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                v.push(root.join("data").join("bridge.db"));
            }
        }
    }
    v.push(PathBuf::from("data/bridge.db"));
    v
}

fn resolve_db() -> Option<PathBuf> {
    candidate_paths().into_iter().find(|p| p.is_file())
}

/// 从 DB 路径反推正式版主程序：`.../<安装目录>/data/bridge.db` → `.../<安装目录>/<exe>`。
fn official_exe_from(db: &Path) -> Option<PathBuf> {
    let dir = db.parent()?.parent()?;
    let exe = dir.join(OFFICIAL_EXE);
    exe.is_file().then_some(exe)
}

/// 只读刷新一次。任何失败都变成界面上可读的 `error`，不 panic、不阻塞。
pub fn load() -> Snapshot {
    let mut s = Snapshot {
        official_gui_open: official_gui_open(),
        refreshed_at: local_clock(),
        ..Default::default()
    };

    // 独立版：强制用自带数据空间，即使库还没生成也不去读正式版的历史库，
    // 否则界面读 A、worker 写 B，两边会对不上。
    let db = if has_bundled_backend() {
        bundled_db()
    } else {
        resolve_db()
    };
    let Some(db) = db else {
        s.error = Some(
            "未找到 bridge.db。请先按 F5 启动一次转发，\n或设置环境变量 MIAOMIAO_BRIDGE_DB 指向数据库文件。".to_owned(),
        );
        return s;
    };
    s.db_path = Some(db.to_string_lossy().into_owned());
    s.official_exe = official_exe_from(&db).map(|p| p.to_string_lossy().into_owned());

    if !db.is_file() {
        // 全新安装、还没转发过：正常空态，不要报红。
        return s;
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = match Connection::open_with_flags(&db, flags) {
        Ok(c) => c,
        Err(e) => {
            s.error = Some(format!("只读打开数据库失败：{e}\n界面会自动重试。"));
            return s;
        }
    };
    // 连接级保险：即便将来误加了写语句，SQLite 也会直接拒绝。
    let _ = conn.execute_batch("PRAGMA query_only = ON;");

    match data::read_all(&conn, &local_date()) {
        Ok(r) => {
            s.kpi = r.kpi;
            s.flow = r.flow;
            s.entries = r.entries;
            s.chat_name = if r.chat_name.is_empty() {
                OFFICIAL_TITLE.to_owned()
            } else {
                r.chat_name
            };
        }
        Err(e) => {
            s.error = Some(format!("读取数据库失败：{e}\n可能是正式版正在写库，界面会自动重试。"));
        }
    }
    s
}

/// 底栏只显示末两级路径，避免长路径把状态栏挤爆。
pub fn display_path(p: &str) -> String {
    let path = Path::new(p);
    let file = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = path
        .parent()
        .and_then(|d| d.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if parent.is_empty() {
        file
    } else {
        format!("{parent}/{file}")
    }
}

/// 独立版的配置文件（安装目录根下，用户可直接编辑钉钉群 / 路由规则）。
pub fn config_file() -> Option<String> {
    let p = exe_dir()?.join("config.yaml");
    p.is_file().then(|| p.to_string_lossy().into_owned())
}

/// 用系统关联程序打开配置（一般是记事本）。
pub fn open_config() -> bool {
    let Some(file) = config_file() else {
        return false;
    };
    let w = wide(&file);
    let op = wide("open");
    unsafe { ShellExecuteW(ptr::null(), op.as_ptr(), w.as_ptr(), ptr::null(), ptr::null(), 1) > 32 }
}

/// 打开 Python 正式版 GUI（不启动 worker，转发仍由用户在正式版里按 F5）。
pub fn open_official(exe: &str) -> bool {
    let path = PathBuf::from(exe);
    if !path.is_file() {
        return false;
    }
    let dir = path.parent().map(|p| p.to_string_lossy().into_owned());
    let op = wide("open");
    let file = wide(exe);
    let dir_w = dir.map(|d| wide(&d));
    unsafe {
        let r = ShellExecuteW(
            ptr::null(),
            op.as_ptr(),
            file.as_ptr(),
            ptr::null(),
            dir_w.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
            1, // SW_SHOWNORMAL
        );
        r > 32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_include_official_install_dir() {
        let paths = candidate_paths();
        assert!(paths
            .iter()
            .any(|p| p.to_string_lossy().contains("喵喵钉钉-微信消息转发桥")));
        assert!(paths.iter().any(|p| p.ends_with("bridge.db")));
    }

    #[test]
    fn local_date_has_iso_shape() {
        let d = local_date();
        assert_eq!(d.len(), 10);
        assert_eq!(&d[4..5], "-");
        let c = local_clock();
        assert_eq!(c.len(), 8);
    }
}
