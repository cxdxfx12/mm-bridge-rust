//! 构建期把 release 版 GUI 主程序嵌进安装器，产出真正的单文件 Setup.exe。
//!
//! 前置条件：`cargo build --release -p bridge-gui` 必须先执行，否则直接构建失败，
//! 避免打出一个「装完没有主程序」的空安装包。

include!("../../assets/embed_resources.rs");

fn main() {
    embed(
        "bridge-setup",
        "Setup.exe",
        "MiaoMiao Bridge (Rust) Setup",
    );

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR 缺失"));
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 缺失"));

    // rust/crates/bridge-setup -> rust/target/release/bridge-gui.exe
    let gui_exe = manifest.join("../../target/release/bridge-gui.exe");
    println!("cargo:rerun-if-changed={}", gui_exe.display());

    assert!(
        gui_exe.is_file(),
        "找不到 {} —— 请先执行 cargo build --release -p bridge-gui",
        gui_exe.display()
    );

    let payload = out.join("payload.bin");
    fs::copy(&gui_exe, &payload).expect("复制 GUI payload 失败");
    println!(
        "cargo:warning=已嵌入 GUI payload：{} 字节",
        fs::metadata(&payload).unwrap().len()
    );
}
