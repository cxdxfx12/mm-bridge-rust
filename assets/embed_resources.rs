// 构建期共享片段：给 Windows exe 嵌入图标 + VS_VERSIONINFO。
//
// 由 `bridge-gui/build.rs` 与 `bridge-setup/build.rs` 用 `include!` 引入，
// 两个 crate 的 build.rs 路径都是 `rust/crates/<crate>/build.rs`，
// 所以 `../../assets/app.ico` 指向同一份图标。
//
// 刻意不引第三方 crate：直接调用 Windows SDK 的 rc.exe 生成 .res，
// 再用 `cargo:rustc-link-arg` 交给 link.exe。找不到 rc.exe 时只警告，
// 不阻断构建（CI / 非 Windows SDK 环境仍能编译出无图标的 exe）。
//
// 注意：这里用普通 `//` 注释而不是 `//!`，因为文件是被 include! 展开的，
// 内部文档注释在展开位置不合法。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 从常见安装位置找 64 位 rc.exe；返回 None 时调用方应跳过资源嵌入。
fn find_rc() -> Option<PathBuf> {
    // 1) 环境变量（VS 开发者命令行通常已设置）
    if let Ok(sdk) = env::var("WindowsSdkDir") {
        if let Some(p) = newest_rc_in(Path::new(&sdk).join("bin")) {
            return Some(p);
        }
    }
    // 2) 标准 Windows Kits 安装目录
    for root in [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ] {
        if let Some(p) = newest_rc_in(PathBuf::from(root)) {
            return Some(p);
        }
    }
    // 3) PATH 上直接有 rc.exe
    let p = PathBuf::from("rc.exe");
    if Command::new(&p).arg("/?").output().is_ok() {
        return Some(p);
    }
    None
}

fn newest_rc_in(bin: PathBuf) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&bin) {
        for e in entries.flatten() {
            let p = e.path().join("x64").join("rc.exe");
            if p.is_file() {
                candidates.push(p);
            }
        }
    }
    candidates.sort();
    candidates.pop() // 版本号字典序最大 = 最新 SDK
}

fn rc_text(internal_name: &str, original_exe: &str, description: &str) -> String {
    // 全部用 ASCII，避免 rc.exe 在中文代码页环境下把 VERSIONINFO 字符串读乱。
    format!(
        r#"1 ICON "app.ico"
1 VERSIONINFO
FILEVERSION 0,3,0,0
PRODUCTVERSION 0,3,0,0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "080404b0"
    BEGIN
      VALUE "CompanyName", "MiaoMiao Bridge"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "0.4.0.0"
      VALUE "InternalName", "{internal_name}"
      VALUE "LegalCopyright", "Copyright (C) 2026 MiaoMiao Bridge"
      VALUE "OriginalFilename", "{original_exe}"
      VALUE "ProductName", "MiaoMiao DingTalk-WeChat Bridge (Rust)"
      VALUE "ProductVersion", "0.4.0.0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x804, 1200
  END
END
"#
    )
}

/// 在调用它的 crate 的 build.rs 中执行。
pub fn embed(internal_name: &str, original_exe: &str, description: &str) {
    if !cfg!(target_os = "windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("..").join("..").join("assets").join("app.ico");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    if !icon.is_file() {
        println!("cargo:warning=缺少 {}，跳过图标嵌入", icon.display());
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=未找到 Windows SDK rc.exe，跳过图标嵌入（exe 仍可正常构建）");
        return;
    };

    // 把图标复制到 OUT_DIR，.rc 里只写相对文件名，彻底绕开路径转义/中文路径问题。
    if fs::copy(&icon, out.join("app.ico")).is_err() {
        println!("cargo:warning=复制图标到 OUT_DIR 失败，跳过图标嵌入");
        return;
    }
    let rc_file = out.join("app.rc");
    let res_file = out.join("app.res");
    if fs::write(&rc_file, rc_text(internal_name, original_exe, description)).is_err() {
        return;
    }

    let status = Command::new(&rc)
        .current_dir(&out)
        .args(["/nologo", "/fo", "app.res", "app.rc"])
        .status();
    match status {
        Ok(s) if s.success() && res_file.is_file() => {
            println!("cargo:rustc-link-arg={}", res_file.display());
            println!(
                "cargo:warning=已嵌入图标与版本资源：{} / {}",
                internal_name, original_exe
            );
        }
        Ok(s) => println!("cargo:warning=rc.exe 返回 {s}，跳过图标嵌入"),
        Err(e) => println!("cargo:warning=调用 rc.exe 失败：{e}，跳过图标嵌入"),
    }
    println!("cargo:rerun-if-changed={}", icon.display());
}
