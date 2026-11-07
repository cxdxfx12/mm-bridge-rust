//! Rust 独立版安装器（单文件、自解压）。
//!
//! 0.3.0 起安装包自带完整 Python 转发引擎：
//! - `rust/assets/pack_backend.py` 把裁剪后的 runtime + 引擎源码打成压缩归档，
//!   append 在本 exe 尾部（PE overlay）。Windows 加载器忽略这段数据，
//!   双击时由 `bundle.rs` 从自身读出、校验 CRC32、解压到 `<安装目录>/backend/`；
//! - 目标机器**不需要预先安装 Python 0.63 正式版**；配置与数据库也落在
//!   本安装目录自己的 `config.yaml` / `data/`，不碰正式版任何文件；
//! - 仍是 windows 子系统（没有控制台），所有子进程带 `CREATE_NO_WINDOW`；
//! - 输出走安全的 `out()` + `%TEMP%` 日志，GUI 子系统下不会 panic；
//! - 卸载入口写的是「去掉 overlay 的 stub」，不把 150 MB 引擎再复制一份。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bundle;

use std::collections::hash_map::RandomState;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

const EXE_NAME: &str = "bridge-gui.exe";
const UNINSTALL_NAME: &str = "卸载.exe";
const DIR_NAME: &str = "MiaoMiaoBridgeRustPreview"; // 内部目录名保持不变，便于平滑升级
const FOLDER_DISPLAY: &str = "喵喵钉钉-微信消息转发桥 Rust独立版";
const LINK_NAME: &str = "喵喵钉钉-微信消息转发桥（独立版）.lnk";
const LINK_UNINSTALL: &str = "卸载 Rust独立版.lnk";
const APP_NAME: &str = "喵喵钉钉-微信消息转发桥 · Rust 独立版";
const VERSION: &str = "0.5.0";
const REG_KEY: &str = "MiaoMiaoBridgeRustPreview";
const PROGRESS_CAPTION: &str = "喵喵转发桥 · 正在安装内置引擎";
const OFFICIAL_DIR: &str = "喵喵钉钉-微信消息转发桥";

// 0.1 / 0.2 版留下的旧快捷方式，安装时顺手清掉，避免桌面出现多个入口。
const OLD_FOLDER_DISPLAYS: [&str; 2] = [
    "喵喵钉钉-微信消息转发桥 Rust预览版",
    "喵喵钉钉-微信消息转发桥 Rust版",
];
const OLD_LINK_NAMES: [&str; 2] = [
    "喵喵钉钉-微信消息转发桥（Rust预览版）.lnk",
    "喵喵钉钉-微信消息转发桥（Rust版）.lnk",
];

/// build.rs 嵌入的 release 版 GUI 主程序。
const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(hwnd: *const std::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    fn FindWindowW(class: *const u16, name: *const u16) -> *mut std::ffi::c_void;
    fn PostMessageW(hwnd: *mut std::ffi::c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
}

const WM_CLOSE: u32 = 0x0010;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 图形提示框：双击安装包的用户没有控制台可读，成败都必须看得见。
fn message_box(caption: &str, text: &str, kind: u32) {
    let c = wide(caption);
    let t = wide(text);
    unsafe {
        MessageBoxW(std::ptr::null(), t.as_ptr(), c.as_ptr(), kind);
    }
}

/// 解压 4600+ 文件要几十秒，双击安装时给一个可见的等待提示。
/// 它在独立线程里跑 MessageBox，主线程解压完用 WM_CLOSE 关掉。
fn show_progress() {
    std::thread::spawn(|| {
        message_box(
            PROGRESS_CAPTION,
            "正在安装内置转发引擎…\n\n首次安装需展开约 350 MB 的 Python 运行时与 OCR 组件，\n请稍候，完成后本提示会自动关闭。",
            0x40, // MB_ICONINFORMATION
        );
    });
}

fn close_progress() {
    let title = wide(PROGRESS_CAPTION);
    unsafe {
        let h = FindWindowW(std::ptr::null(), title.as_ptr());
        if !h.is_null() {
            PostMessageW(h, WM_CLOSE, 0, 0);
        }
    }
}

fn setup_log_path() -> PathBuf {
    env::temp_dir().join("MiaoMiaoBridgeRustSetup.log")
}

/// 安全输出：GUI 子系统里 stdout 是无效句柄，直接 `println!` 会 panic。
/// 这里忽略写失败，并把每一步落到日志文件，静默安装也能复查。
fn out(s: &str) {
    let _ = writeln!(io::stdout(), "{s}");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(setup_log_path()) {
        let _ = writeln!(f, "{s}");
    }
}

/// 所有控制台子进程都从这里创建，统一挂 CREATE_NO_WINDOW。
#[cfg(windows)]
fn silent(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(program);
    c.creation_flags(0x0800_0000);
    c
}

#[cfg(not(windows))]
fn silent(program: &str) -> Command {
    Command::new(program)
}

fn launch_app(exe: &Path) -> io::Result<()> {
    silent(exe.to_string_lossy().as_ref())
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .spawn()?;
    Ok(())
}

struct Opts {
    uninstall: bool,
    external: bool,
    silent: bool,
    no_shortcut: bool,
    purge: bool,
    dir: PathBuf,
}

fn default_dir() -> PathBuf {
    let base = env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir());
    base.join("Programs").join(DIR_NAME)
}

fn parse() -> Opts {
    let mut o = Opts {
        uninstall: false,
        external: false,
        silent: false,
        no_shortcut: false,
        purge: false,
        dir: default_dir(),
    };
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--uninstall" => o.uninstall = true,
            "--external" => o.external = true,
            "--silent" => o.silent = true,
            "--no-shortcut" => o.no_shortcut = true,
            "--purge" => o.purge = true,
            "--dir" => {
                if let Some(v) = it.next() {
                    o.dir = PathBuf::from(v);
                }
            }
            _ => {}
        }
    }
    o
}

/// PowerShell 单引号字符串转义。
fn q(s: &str) -> String {
    s.replace('\'', "''")
}

/// 把脚本写成带 BOM 的 UTF-8 临时 .ps1 再执行，避免中文路径在 -Command 下乱码。
fn run_ps(script: &str) -> io::Result<()> {
    let path = env::temp_dir().join(format!("bridge_setup_{}.ps1", std::process::id()));
    let mut text = String::from(
        "\u{feff}$ErrorActionPreference = 'Stop'\n\
         $OutputEncoding = [Text.Encoding]::UTF8\n\
         [Console]::OutputEncoding = [Text.Encoding]::UTF8\n",
    );
    text.push_str(script);
    fs::write(&path, text)?;

    let mut ps = silent("powershell.exe");
    let result = ps
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&path)
        .output();
    let _ = fs::remove_file(&path);
    let outp = result?;

    if !outp.status.success() {
        let err = String::from_utf8_lossy(&outp.stderr);
        return Err(io::Error::other(format!("PowerShell 执行失败：{err}")));
    }
    let msg = String::from_utf8_lossy(&outp.stdout);
    if !msg.trim().is_empty() {
        out(msg.trim());
    }
    Ok(())
}

fn program_paths() -> (String, String, String) {
    let desktop = format!(
        "$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) '{}'\n",
        q(LINK_NAME)
    );
    let folder = format!(
        "$folder = Join-Path ([Environment]::GetFolderPath('Programs')) '{}'\n",
        q(FOLDER_DISPLAY)
    );
    let links = format!(
        "$mainLnk = Join-Path $folder '{}'\n$unLnk = Join-Path $folder '{}'\n",
        q(LINK_NAME),
        q(LINK_UNINSTALL)
    );
    (desktop, folder, links)
}

fn old_shortcut_cleanup() -> String {
    let mut s = String::new();
    for name in OLD_LINK_NAMES {
        s.push_str(&format!(
            "$oldDesk = Join-Path ([Environment]::GetFolderPath('Desktop')) '{}'\n\
             if (Test-Path -LiteralPath $oldDesk) {{ Remove-Item -LiteralPath $oldDesk -Force -ErrorAction SilentlyContinue }}\n",
            q(name)
        ));
    }
    for name in OLD_FOLDER_DISPLAYS {
        s.push_str(&format!(
            "$oldFolder = Join-Path ([Environment]::GetFolderPath('Programs')) '{}'\n\
             if (Test-Path -LiteralPath $oldFolder) {{ Remove-Item -LiteralPath $oldFolder -Recurse -Force -ErrorAction SilentlyContinue }}\n",
            q(name)
        ));
    }
    s
}

fn install_script(dir: &Path) -> String {
    let exe = dir.join(EXE_NAME);
    let uninst = dir.join(UNINSTALL_NAME);
    let (desktop, folder, links) = program_paths();

    format!(
        r#"{desktop}{folder}{links}{cleanup}
$ws = New-Object -ComObject WScript.Shell
New-Item -ItemType Directory -Force -Path $folder | Out-Null

$l1 = $ws.CreateShortcut($desktop)
$l1.TargetPath = '{exe}'
$l1.WorkingDirectory = '{dir}'
$l1.IconLocation = '{exe},0'
$l1.Description = '{app}'
$l1.Save()

$l2 = $ws.CreateShortcut($mainLnk)
$l2.TargetPath = '{exe}'
$l2.WorkingDirectory = '{dir}'
$l2.IconLocation = '{exe},0'
$l2.Description = '{app}'
$l2.Save()

$l3 = $ws.CreateShortcut($unLnk)
$l3.TargetPath = '{uninst}'
$l3.Arguments = '--uninstall --silent --dir "{dir}"'
$l3.WorkingDirectory = '{dir}'
$l3.Description = '卸载 {app}'
$l3.Save()

$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{reg}'
New-Item -Path $key -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayName -Value '{app}' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayVersion -Value '{ver}' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name Publisher -Value 'MiaoMiao Bridge' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name InstallLocation -Value '{dir}' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name UninstallString -Value '"{uninst}" --uninstall --dir "{dir}"' -PropertyType String -Force | Out-Null
Write-Output '  已创建桌面与开始菜单快捷方式、写入卸载信息'
"#,
        desktop = desktop,
        folder = folder,
        links = links,
        cleanup = old_shortcut_cleanup(),
        exe = q(&exe.to_string_lossy()),
        dir = q(&dir.to_string_lossy()),
        uninst = q(&uninst.to_string_lossy()),
        app = q(APP_NAME),
        reg = REG_KEY,
        ver = VERSION,
    )
}

fn uninstall_script() -> String {
    let (desktop, folder, links) = program_paths();
    format!(
        r#"{desktop}{folder}{links}
foreach ($p in @($desktop, $mainLnk, $unLnk)) {{
    if (Test-Path -LiteralPath $p) {{ Remove-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue }}
}}
if (Test-Path -LiteralPath $folder) {{ Remove-Item -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue }}
{cleanup}
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{reg}'
if (Test-Path $key) {{ Remove-Item $key -Recurse -Force -ErrorAction SilentlyContinue }}
Write-Output '  已清理快捷方式与卸载信息'
"#,
        desktop = desktop,
        folder = folder,
        links = links,
        cleanup = old_shortcut_cleanup(),
        reg = REG_KEY,
    )
}

/// 本机若已装 Python 正式版，继承它的网关 token —— 手机端不用重新填。
fn inherited_gateway_token() -> Option<String> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(b) = env::var(key) {
            paths.push(PathBuf::from(b).join(OFFICIAL_DIR).join("config.yaml"));
        }
    }
    if let Ok(a) = env::var("APPDATA") {
        paths.push(PathBuf::from(a).join(OFFICIAL_DIR).join("config.yaml"));
    }
    paths.push(PathBuf::from(
        r"C:\Program Files\喵喵钉钉-微信消息转发桥\config.yaml",
    ));
    for p in paths {
        let Ok(txt) = fs::read_to_string(&p) else { continue };
        for line in txt.lines() {
            let Some(rest) = line.trim().strip_prefix("gateway_token:") else {
                continue;
            };
            if let Some(a) = rest.find('"') {
                if let Some(b) = rest[a + 1..].find('"') {
                    let v = &rest[a + 1..a + 1 + b];
                    if !v.is_empty() && !v.contains("改成") {
                        return Some(v.to_owned());
                    }
                }
            }
        }
    }
    None
}

/// 用标准库的随机哈希种子生成 48 位十六进制 token（Windows 上种子来自系统熵源）。
fn random_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut seed = nanos ^ (std::process::id() as u64).rotate_left(17);
    let mut out = String::with_capacity(48);
    for i in 0..3u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(seed ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        seed = h.finish();
        out.push_str(&format!("{seed:016x}"));
    }
    out
}

/// 纯文本变换：只把空引号里填上 token，缩进 / 键名 / 行尾注释全部原样保留。
/// 返回 None 表示这一行已经有值（或压根没有这一行），不需要改动。
fn with_gateway_token(txt: &str, token: &str) -> Option<String> {
    let mut off = 0usize;
    let mut line_start = None;
    for line in txt.split_inclusive('\n') {
        if line.trim_start().starts_with("gateway_token:") {
            line_start = Some(off);
            break;
        }
        off += line.len();
    }
    let start = line_start?;
    let line = &txt[start..txt[start..].find('\n').map(|n| start + n).unwrap_or(txt.len())];
    let q1 = line.find('"')?;
    let q2 = q1 + 1 + line[q1 + 1..].find('"')?;
    if !line[q1 + 1..q2].trim().is_empty() {
        return None;
    }
    let mut new = String::with_capacity(txt.len() + token.len());
    new.push_str(&txt[..start + q1 + 1]);
    new.push_str(token);
    new.push_str(&txt[start + q2..]);
    Some(new)
}

/// 模板配置里的 gateway_token 是空的（不能把本机凭证带给别人）。
/// 采集端自检要求它非空，所以首次安装时必须补上：本机继承旧值，干净机器随机生成。
fn ensure_gateway_token(dir: &Path, config_created: bool) -> io::Result<String> {
    if !config_created {
        return Ok(String::new());
    }
    let p = dir.join("config.yaml");
    let txt = fs::read_to_string(&p)?;
    // MIAOMIAO_FRESH_TOKEN=1 可强制生成新 token（换手机 / 不沿用旧配置的场景，
    // 也用于验证「一台没装过正式版的干净机器」这条分支）。
    let inherited = if env::var_os("MIAOMIAO_FRESH_TOKEN").is_some() {
        None
    } else {
        inherited_gateway_token()
    };
    let (token, note) = match inherited {
        Some(t) => (t, "继承本机已有网关 token（手机端无需改动）"),
        None => (random_token(), "已生成随机网关 token（手机端需填同一个值）"),
    };
    match with_gateway_token(&txt, &token) {
        Some(new) => {
            fs::write(&p, new)?;
            Ok(note.to_owned())
        }
        None => Ok(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_keeps_yaml_indentation_and_comment() {
        let src = "dingtalk:\n  mode: \"android\"\n  gateway_token: \"\"  # 必填\n";
        let out = with_gateway_token(src, "abc123").unwrap();
        assert_eq!(
            out,
            "dingtalk:\n  mode: \"android\"\n  gateway_token: \"abc123\"  # 必填\n"
        );
    }

    #[test]
    fn inject_does_not_overwrite_existing_token() {
        let src = "dingtalk:\n  gateway_token: \"existing-token\"\n";
        assert!(with_gateway_token(src, "new").is_none());
    }

    #[test]
    fn random_token_is_long_and_varies() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
    }
}

/// 释放内置的 Python 转发引擎。返回给用户看的一行摘要。
fn install_backend(o: &Opts) -> io::Result<String> {
    let Some(b) = bundle::read_own()? else {
        out(">> 本安装包未附带引擎归档（开发构建），保留「依赖正式版」模式");
        return Ok("未内置引擎".into());
    };
    out(&format!(
        ">> 内置引擎归档：{}，CRC {:08x}",
        bundle::describe(&b),
        b.checksum
    ));

    if bundle::already_installed(&b, &o.dir) {
        out(">> 内置引擎已是同一版本，跳过解压");
        return Ok(format!("内置引擎 {} · 已是最新", bundle::describe(&b)));
    }

    if !o.silent {
        show_progress();
    }
    bundle::clear_backend(&o.dir)?;
    fs::create_dir_all(o.dir.join("backend"))?;
    let mut progress = |done: usize, total: usize| {
        out(&format!("   解压进度 {done}/{total}"));
    };
    let result = bundle::extract(&b, &o.dir, &mut progress);
    if !o.silent {
        close_progress();
    }
    let st = result?;
    out(&format!(
        ">> 已展开 {} 个文件（{} MB），config.yaml {}",
        st.files,
        st.bytes / 1048576,
        if st.config_created { "已生成" } else { "保留用户版本" }
    ));
    let token_note = ensure_gateway_token(&o.dir, st.config_created)?;
    if !token_note.is_empty() {
        out(&format!(">> {token_note}"));
    }
    fs::create_dir_all(o.dir.join("data"))?;
    Ok(format!("内置引擎 {} · 已展开", bundle::describe(&b)))
}

/// 卸载入口写「去掉 overlay 的 stub」，避免复制 150 MB 归档到安装目录 / 临时目录。
fn write_uninstall_stub(target: &Path) -> io::Result<()> {
    let me = env::current_exe()?;
    let len = fs::metadata(&me)?.len();
    let take = bundle::stub_len()?.unwrap_or(len);
    let mut f = File::open(&me)?;
    let mut buf = vec![0u8; take as usize];
    f.read_exact(&mut buf)?;
    fs::write(target, buf)
}

fn install(o: &Opts) -> io::Result<()> {
    out(&format!("=== {APP_NAME} 安装程序 v{VERSION} ==="));
    out("提示：本版内置完整 Python 转发引擎，目标机器无需预先安装 0.63 正式版。\n");
    out(&format!("安装目录：{}", o.dir.display()));

    fs::create_dir_all(&o.dir)?;

    let exe = o.dir.join(EXE_NAME);
    let same = exe.exists()
        && fs::read(&exe).map(|b| b.as_slice() == PAYLOAD).unwrap_or(false);
    if same {
        out(">> 主程序已是最新版本，跳过覆盖");
    } else {
        out(&format!(">> 释放主程序（{} 字节）…", PAYLOAD.len()));
        let old = o.dir.join("bridge-gui.old.exe");
        if exe.exists() {
            let _ = fs::remove_file(&old);
            if fs::rename(&exe, &old).is_err() {
                return Err(io::Error::other(
                    "旧版 bridge-gui.exe 正在运行，请先关闭窗口再重新安装",
                ));
            }
        }
        fs::write(&exe, PAYLOAD)?;
        let _ = fs::remove_file(&old);
    }

    let engine = install_backend(o)?;

    let uninst = o.dir.join(UNINSTALL_NAME);
    if env::current_exe()? != uninst {
        write_uninstall_stub(&uninst)?;
    }
    out(&format!(">> 已写入卸载入口 {}", uninst.display()));

    if !o.no_shortcut {
        out(">> 创建快捷方式…");
        run_ps(&install_script(&o.dir))?;
    } else {
        out(">> 跳过快捷方式（--no-shortcut）");
    }

    out(&format!("\n安装完成。{engine}"));
    out(&format!("主程序：{}", exe.display()));
    out(&format!(
        "数据目录：{}（卸载默认保留，--purge 才删除）",
        o.dir.join("data").display()
    ));
    out(&format!("卸载方式：开始菜单「{FOLDER_DISPLAY}」→ 卸载，或运行 {UNINSTALL_NAME} --uninstall"));
    Ok(())
}

/// 卸载时默认保留 data/（里面有用户的 bridge.db），只有 --purge 才清。
fn remove_install_dir(dir: &Path, purge: bool) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        let keep = !purge
            && p.file_name()
                .map(|n| n.eq_ignore_ascii_case("data"))
                .unwrap_or(false);
        if keep {
            out(&format!(">> 保留用户数据：{}", p.display()));
            continue;
        }
        if p.is_dir() {
            fs::remove_dir_all(&p)?;
        } else {
            let _ = fs::remove_file(&p);
        }
    }
    if purge || fs::read_dir(dir)?.next().is_none() {
        let _ = fs::remove_dir(dir);
    }
    Ok(())
}

fn uninstall(o: &Opts) -> io::Result<()> {
    if !o.external {
        let self_exe = env::current_exe()?;
        let temp = env::temp_dir().join(format!("bridge_uninstall_{}.exe", std::process::id()));
        fs::copy(&self_exe, &temp)?;
        out("已启动独立卸载进程，本窗口可以关闭。");
        let mut c = silent(temp.to_string_lossy().as_ref());
        c.args(["--uninstall", "--external", "--silent", "--dir"])
            .arg(&o.dir);
        if o.purge {
            c.arg("--purge");
        }
        c.spawn()?;
        return Ok(());
    }

    out(&format!("=== 卸载 {APP_NAME} ==="));
    out(&format!("目标目录：{}", o.dir.display()));
    let _ = run_ps(&uninstall_script());

    if o.dir.exists() {
        remove_install_dir(&o.dir, o.purge)?;
    }
    out("卸载完成。");

    // 延迟删除临时副本自身（当前进程退出后执行）。
    if let Ok(me) = env::current_exe() {
        let cmd = format!(
            "Start-Sleep -Milliseconds 900; Remove-Item -LiteralPath '{}' -Force -ErrorAction SilentlyContinue",
            q(&me.to_string_lossy())
        );
        let _ = silent("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
            .spawn();
    }
    Ok(())
}

fn main() -> ExitCode {
    let o = parse();
    let uninstalling = o.uninstall;
    let result = if uninstalling { uninstall(&o) } else { install(&o) };

    let code = match &result {
        Ok(()) => 0,
        Err(e) => {
            out(&format!("\n失败：{e}"));
            1
        }
    };

    // 双击安装包的用户通常不看控制台：成败都用图形提示框明确反馈，
    // 并且安装成功后直接把主界面启动起来。
    if !o.silent {
        match result {
            Ok(()) if !uninstalling => {
                let exe = o.dir.join(EXE_NAME);
                let launched = launch_app(&exe).is_ok();
                let text = if launched {
                    format!(
                        "安装完成，主界面已启动。\n\n本版已内置转发引擎，无需另外安装 Python 正式版。\n安装目录：{}\n\n在界面上点「开始转发」（或按 F5）即可运行。",
                        o.dir.display()
                    )
                } else {
                    format!(
                        "安装完成，但自动启动主界面失败。\n\n请手动运行：\n{}\n\n安装目录：{}",
                        exe.display(),
                        o.dir.display()
                    )
                };
                message_box("安装完成", &text, if launched { 0x40 } else { 0x30 });
            }
            Ok(()) => message_box(
                "卸载完成",
                if o.purge {
                    "已移除独立版及其数据目录。"
                } else {
                    "已移除独立版。\n用户数据目录已保留（安装目录\\data），需要彻底清理可用 --purge。"
                },
                0x40,
            ),
            Err(e) => message_box(
                "操作失败",
                &format!("{e}\n\n请关闭正在运行的 Rust 版窗口后重试，或把这段文字发给开发者。"),
                0x10,
            ),
        }
    }
    ExitCode::from(code as u8)
}
