//! 启动自检：单实例置前、启动/崩溃日志、可见错误弹窗。
//!
//! release 版是 windows 子系统（没有控制台），一旦初始化失败，用户屏幕上什么都不会有，
//! 表现为「双击了没反应 / 启动不了」。这里把关键事件写进安装目录的 startup.log /
//! crash.log，并用 MessageBox 把失败原因显示出来，同时用命名互斥体保证重复双击时
//! 把已有窗口拉到前台，而不是静默地什么也不做。

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::ptr;

/// 与 Python 正式版区分开的唯一窗口标题，供单实例查找使用。
pub const WINDOW_TITLE: &str = "喵喵钉钉-微信消息转发桥 · Rust 版";

/// 0.1.0 预览版的旧标题：升级覆盖时若旧窗口还开着，也要能把用户送回已有实例。
const OLD_WINDOW_TITLE: &str = "喵喵钉钉-微信消息转发桥 · Rust 预览版";

const MUTEX_NAME: &str = "MiaoMiaoBridgeRustPreviewSingleton";
const ERROR_ALREADY_EXISTS: u32 = 183;
const SW_RESTORE: i32 = 9;
const MB_ICONERROR: u32 = 0x10;

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
    fn CreateMutexW(attrs: *const c_void, initial_owner: i32, name: *const u16) -> *mut c_void;
    fn GetLastError() -> u32;
    fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, size: u32) -> u32;
    fn GetLocalTime(time: *mut SystemTime);
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, name: *const u16) -> *mut c_void;
    fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
    fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 直接用系统本地时间，避免自己算日历把月份/年份算错。
fn stamp() -> String {
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// 日志放在 exe 同目录（安装目录 `%LOCALAPPDATA%` 可写），失败时回落 TEMP。
fn log_path(name: &str) -> PathBuf {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetModuleFileNameW(ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    let dir = if n > 0 && (n as usize) < buf.len() {
        let exe = String::from_utf16_lossy(&buf[..n as usize]);
        PathBuf::from(exe).parent().map(|p| p.to_path_buf())
    } else {
        None
    };
    dir.unwrap_or_else(std::env::temp_dir).join(name)
}

fn append(name: &str, line: &str) {
    let path = log_path(name);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {}", stamp(), line);
    }
}

pub fn log(line: &str) {
    append("startup.log", line);
}

pub fn log_file(name: &str) -> PathBuf {
    log_path(name)
}

pub fn message_box(caption: &str, text: &str, kind: u32) {
    let c = wide(caption);
    let t = wide(text);
    unsafe {
        MessageBoxW(ptr::null_mut(), t.as_ptr(), c.as_ptr(), kind);
    }
}

/// panic 时写 crash.log 并弹框，避免 release 版静默消失。
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{info}");
        append("crash.log", &msg);
        let detail = format!(
            "界面遇到异常需要退出。\n\n{msg}\n\n详细信息已写入安装目录的 crash.log，请把该文件发给开发者。"
        );
        message_box("程序异常", &detail, MB_ICONERROR);
    }));
}

/// 已有实例时把它的窗口拉到前台，返回 true 表示本进程应当直接退出。
pub fn forward_to_existing() -> bool {
    // 并列验证 / 灰度对照用：设了它就不参与单实例互斥。
    // 日常双击不会设置，行为完全不变。
    if std::env::var_os("MIAOMIAO_ALLOW_MULTI").is_some() {
        log("MIAOMIAO_ALLOW_MULTI=1，跳过单实例检查（并列验证）");
        return false;
    }
    unsafe {
        let name = wide(MUTEX_NAME);
        let mutex = CreateMutexW(ptr::null(), 0, name.as_ptr());
        if mutex.is_null() {
            log("创建单实例互斥体失败，继续启动");
            return false;
        }
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }

        let title = wide(WINDOW_TITLE);
        let old_title = wide(OLD_WINDOW_TITLE);
        // 首个实例可能刚创建互斥体、窗口还没建好，给 2 秒窗口期。
        for _ in 0..20 {
            let mut hwnd = FindWindowW(ptr::null(), title.as_ptr());
            if hwnd.is_null() {
                hwnd = FindWindowW(ptr::null(), old_title.as_ptr());
            }
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                log("已有实例在运行，已把窗口置前后退出本次启动");
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        log("已有互斥体但始终找不到窗口，改为启动新实例");
        false
    }
}
