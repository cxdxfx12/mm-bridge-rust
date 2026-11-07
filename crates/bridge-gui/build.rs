//! 给 bridge-gui.exe 嵌入应用图标与版本信息。
include!("../../assets/embed_resources.rs");

fn main() {
    embed(
        "bridge-gui",
        "bridge-gui.exe",
        "MiaoMiao DingTalk to WeChat Message Forwarding Bridge",
    );
}
