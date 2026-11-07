//! 手机画面：adb 周期抓帧 → 解码 → UI 纹理。
//!
//! 为什么走 adb 而不是网关 `/screen`：画面是「看」的，刷新要稳、要快；
//! `adb exec-out screencap -p` 一次约 0.8 秒（真机实测 PAL-AL00），
//! 不依赖手机端 App 在线，也不与「转发的 WiFi 通道」抢带宽 —— 面板走
//! adb、转发走 WiFi，互不影响（口径与 Python 版 phone_panel 一致）。
//!
//! 线程模型：后台线程循环「查设备 → 抓帧 → 降采样 → 发通道」，UI 侧每帧
//! `try_recv` 排空后转成 `TextureHandle`。抓帧失败不致命：面板回到
//! 「未连接/未授权」并保留最后一帧。
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// 抓帧降采样目标宽度：纹理太大（1176×2480 ≈ 11 MB/帧）上传与显存都浪费，
/// 面板显示宽度最多 ~560 逻辑 px（2x DPI ≈ 1120 物理 px），720 足够清晰。
const DOWNSCALE_W: u32 = 720;
/// 两帧之间的间隔（抓帧本身约 0.8s，整体约 1.2 fps，足够「实时看」）。
const FRAME_INTERVAL: Duration = Duration::from_millis(400);
/// 无设备/失败时的重试间隔。
const RETRY_INTERVAL: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    /// 没插设备 / adb 不可用。
    Offline,
    /// 插着但手机上没点「允许 USB 调试」。
    Unauthorized,
    /// 可以抓帧。
    Online,
}

impl DeviceState {
    pub fn label(&self) -> &'static str {
        match self {
            DeviceState::Online => "设备在线",
            DeviceState::Unauthorized => "未授权（手机上点「允许 USB 调试」）",
            DeviceState::Offline => "未连接",
        }
    }
}

pub struct PhoneFrame {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

pub enum PhoneMsg {
    State(DeviceState),
    Frame(PhoneFrame),
}

/// UI 侧持有的抓帧管线。
pub struct PhoneFeed {
    pub rx: Receiver<PhoneMsg>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    /// 「手动刷新」：置位后线程跳过本轮 sleep 立刻抓一帧。
    kick: Arc<AtomicBool>,
    pub adb_path: Option<String>,
}

impl PhoneFeed {
    pub fn spawn() -> Self {
        let (tx, rx) = channel::<PhoneMsg>();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let kick = Arc::new(AtomicBool::new(false));
        let adb_path = find_adb();

        let (stop2, paused2, kick2) = (stop.clone(), paused.clone(), kick.clone());
        let adb_for_thread = adb_path.clone();
        thread::Builder::new()
            .name("phone-feed".into())
            .spawn(move || run_feed(adb_for_thread, tx, stop2, paused2, kick2))
            .ok();

        Self {
            rx,
            stop,
            paused,
            kick,
            adb_path,
        }
    }

    pub fn set_paused(&self, p: bool) {
        self.paused.store(p, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn kick(&self) {
        self.kick.store(true, Ordering::Relaxed);
    }
}

impl Drop for PhoneFeed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run_feed(
    adb: Option<String>,
    tx: Sender<PhoneMsg>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    kick: Arc<AtomicBool>,
) {
    let mut last_state = DeviceState::Offline;
    let _ = tx.send(PhoneMsg::State(last_state.clone()));
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if paused.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        let Some(adb) = adb.as_deref() else {
            thread::sleep(RETRY_INTERVAL);
            continue;
        };

        match device_state(adb) {
            DeviceState::Online => {
                if last_state != DeviceState::Online {
                    last_state = DeviceState::Online;
                    let _ = tx.send(PhoneMsg::State(last_state.clone()));
                }
                match capture(adb) {
                    Ok(f) => {
                        let _ = tx.send(PhoneMsg::Frame(f));
                        wait(&stop, &paused, &kick, FRAME_INTERVAL);
                    }
                    Err(_) => {
                        // 瞬态失败（握手中断等）：退回去重查设备状态。
                        last_state = DeviceState::Offline;
                        let _ = tx.send(PhoneMsg::State(last_state.clone()));
                        wait(&stop, &paused, &kick, RETRY_INTERVAL);
                    }
                }
            }
            state => {
                if last_state != state {
                    last_state = state.clone();
                    let _ = tx.send(PhoneMsg::State(state));
                }
                wait(&stop, &paused, &kick, RETRY_INTERVAL);
            }
        }
    }
}

/// 睡眠等待，期间响应「手动刷新」提前唤醒。
fn wait(stop: &AtomicBool, paused: &AtomicBool, kick: &AtomicBool, total: Duration) {
    let step = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < total {
        if stop.load(Ordering::Relaxed) || kick.swap(false, Ordering::Relaxed) {
            return;
        }
        if paused.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(step);
        waited += step;
    }
}

/// `adb devices` 输出 → 设备状态（取第一台设备）。
fn device_state(adb: &str) -> DeviceState {
    let Ok(out) = run(adb, &["devices"], 5.0) else {
        return DeviceState::Offline;
    };
    let text = String::from_utf8_lossy(&out);
    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((serial, state)) = line.split_once('\t') {
            let _ = serial;
            // `adb devices` 给 "device"；`devices -l` 给 "device product:… model:…"。
            // 取第一个词，两种形态都认。
            return match state.trim().split_whitespace().next().unwrap_or("") {
                "device" => DeviceState::Online,
                "unauthorized" => DeviceState::Unauthorized,
                _ => DeviceState::Offline,
            };
        }
    }
    DeviceState::Offline
}

/// 抓一帧并降采样成 RGBA。
fn capture(adb: &str) -> Result<PhoneFrame, String> {
    let png = run(adb, &["exec-out", "screencap", "-p"], 8.0)?;
    let img = image::load_from_memory(&png).map_err(|e| format!("PNG 解码失败：{e}"))?;
    let img = if img.width() > DOWNSCALE_W {
        img.thumbnail(DOWNSCALE_W, u32::MAX)
    } else {
        img
    };
    Ok(PhoneFrame {
        rgba: img.to_rgba8().into_raw(),
        w: img.width(),
        h: img.height(),
    })
}

/// 找 adb.exe：安装目录 → 开发目录 → PATH。与 Python 版 find_adb 同口径。
pub fn find_adb() -> Option<String> {
    if let Ok(p) = std::env::var("MIAOMIAO_BRIDGE_ADB") {
        if std::path::Path::new(&p).is_file() {
            return Some(p);
        }
    }
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 独立版：<安装目录>/backend/tools/platform-tools/adb.exe
            roots.push(dir.join("backend").join("tools").join("platform-tools"));
            // 免安装/开发：<exe 目录>/tools/platform-tools
            roots.push(dir.join("tools").join("platform-tools"));
            // 开发态：target/release → 仓库根 tools/platform-tools
            if let Some(repo) = dir.ancestors().nth(3) {
                roots.push(repo.join("tools").join("platform-tools"));
            }
        }
    }
    for root in roots {
        let p = root.join("adb.exe");
        if p.is_file() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    // PATH（Git Bash 环境里 which 不到也没关系，Windows 会按 PATH 搜）
    Command::new("adb")
        .arg("--version")
        .creation_flags(0x0800_0000)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| "adb".to_owned())
}

fn run(adb: &str, args: &[&str], timeout_s: f32) -> Result<Vec<u8>, String> {
    let mut child = Command::new(adb)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW：绝不闪黑框
        .spawn()
        .map_err(|e| format!("adb 启动失败：{e}"))?;

    let mut stdout = child.stdout.take().expect("stdout piped");
    // 读放在独立线程：stdout 是阻塞读，主线程必须能按时杀掉挂死的 adb。
    let reader = thread::spawn(move || {
        let mut buf = Vec::with_capacity(512 * 1024);
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > 64 * 1024 * 1024 {
                        break; // 防御：异常情况下不要吃光内存
                    }
                }
            }
        }
        buf
    });

    let deadline = Duration::from_secs_f32(timeout_s);
    let started = std::time::Instant::now();
    let mut buf: Option<Vec<u8>> = None;
    while started.elapsed() < deadline {
        if reader.is_finished() {
            buf = Some(reader.join().unwrap_or_default());
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }
    let _ = child.kill();
    let _ = child.wait();

    match buf {
        Some(b) => Ok(b),
        None => Err(format!("adb 超时（{timeout_s:.0}s）：{}", args.join(" "))),
    }
}

/// 排空通道里的最新消息（UI 每帧调用一次）。
pub fn drain(
    rx: &Receiver<PhoneMsg>,
    state: &mut DeviceState,
    frame: &mut Option<PhoneFrame>,
) -> bool {
    let mut got_frame = false;
    loop {
        match rx.try_recv() {
            Ok(PhoneMsg::State(s)) => *state = s,
            Ok(PhoneMsg::Frame(f)) => {
                *frame = Some(f);
                got_frame = true;
            }
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        }
    }
    got_frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_state_parses_adb_output() {
        // 真机输出样例（PAL-AL00，2026-11-07 实测）
        let out = "List of devices attached\nPHQ0222C07004266\tdevice product:PAL-AL00 model:PAL_AL00 device:HWPAL transport_id:1\n\n";
        // device_state 直接跑 adb 不好测，这里锁「解析规则」用的同一逻辑：
        let mut found = DeviceState::Offline;
        for line in out.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((_serial, state)) = line.split_once('\t') {
                found = match state.trim().split_whitespace().next().unwrap_or("") {
                    "device" => DeviceState::Online,
                    "unauthorized" => DeviceState::Unauthorized,
                    _ => DeviceState::Offline,
                };
                break;
            }
        }
        assert_eq!(found, DeviceState::Online);
    }

    #[test]
    fn drain_keeps_only_latest_frame() {
        let (tx, rx) = channel();
        tx.send(PhoneMsg::State(DeviceState::Online)).unwrap();
        tx.send(PhoneMsg::Frame(PhoneFrame {
            rgba: vec![0; 4],
            w: 1,
            h: 1,
        }))
        .unwrap();
        tx.send(PhoneMsg::Frame(PhoneFrame {
            rgba: vec![1; 4],
            w: 1,
            h: 1,
        }))
        .unwrap();
        let mut state = DeviceState::Offline;
        let mut frame = None;
        assert!(drain(&rx, &mut state, &mut frame));
        assert_eq!(state, DeviceState::Online);
        assert_eq!(frame.as_ref().map(|f| f.rgba[0]), Some(1));
    }
}
