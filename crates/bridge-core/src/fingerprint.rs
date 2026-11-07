//! 消息指纹，与 Python `bridge/fingerprint.py` 逐字节等价。
//!
//! 为什么先迁这个模块：OCR 路径和安卓网关都要算指纹，两边算法一旦漂移，
//! 同一条消息会被采两次、转发两次。仓库里已经踩过「机器人昵称后缀时有时无」
//! 导致重复转发的坑，所以实现只保留一份，并用 golden 测试锁死。
//!
//! 与 Python 的差异处理：`re.sub(r"\s+", ...)` 的 `\s` 比 Rust
//! `char::is_whitespace()` 多覆盖 U+001C..U+001F，下面显式补上，避免边界漂移。

use md5::{Digest, Md5};
use std::collections::HashSet;

/// 机器人昵称后缀。归一化时只去结尾这一处，不去更靠前的字，
/// 否则「签收跟单专员」和「拦截跟单专员」会被误判成同一个机器人。
const ROBOT_SUFFIX: &str = "机器人";

/// 参与近似判重的最短内容骨架长度。短消息一律不做近似去重，
/// 宁可多发一条，也不能吃掉「收到」「好的」这类正常重复发言。
pub const NEAR_MIN_LEN: usize = 30;

#[inline]
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{1f}')
}

/// 发送者归一化，只用于指纹与相似度判定，不改变入库/转发时的展示。
///
/// 去掉所有空白，并剥掉结尾的「机器人」后缀：
/// - 「签收跟单专员 机器人」→「签收跟单专员」
/// - 「签收 跟单专员」     →「签收跟单专员」
pub fn norm_sender(sender: &str) -> String {
    let s: String = sender.chars().filter(|c| !is_py_space(*c)).collect();
    match s.strip_suffix(ROBOT_SUFFIX) {
        Some(stripped) => stripped.to_owned(),
        None => s,
    }
}

/// 发送者 + 内容 → md5 指纹。拼接格式必须与 Python 完全一致：
/// `f"{norm_sender(sender)}|{content}"`，正文不做任何归一化。
pub fn fp_of(sender: &str, content: &str) -> String {
    let key = format!("{}|{}", norm_sender(sender), content);
    let digest = Md5::digest(key.as_bytes());
    let mut out = String::with_capacity(32);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// 消息单元指纹（等价于 Python `unit_fp`）。
pub fn unit_fp(unit: &serde_like::Unit) -> String {
    fp_of(unit.sender, unit.content)
}

/// 轻量单元视图，避免为了算指纹就把 serde 拉进核心 crate。
pub mod serde_like {
    pub struct Unit<'a> {
        pub sender: &'a str,
        pub content: &'a str,
    }
}

#[inline]
fn is_piece_sep(c: char) -> bool {
    is_py_space(c)
        || matches!(
            c,
            ':' | '：' | ',' | '，' | '、' | ';' | '；' | '·' | '•' | '→' | '|' | '｜'
        )
}

/// 把正文切成「不计顺序的字面片段」。块内保序、块间不计顺序。
///
/// 依据：同一张钉钉机器人卡片在手机上会横排/一格一行两种渲染，
/// 信息完全一样只是单元格顺序不同；用集合比较才能判为同一条消息。
pub fn content_pieces(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in content.chars() {
        if is_piece_sep(c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[inline]
fn is_key_sep(c: char) -> bool {
    // 注意：content_key 的分隔符集合**不含** '→'，与 content_pieces 略有差异，
    // 这是 Python 侧两个正则的原有行为，必须原样保留。
    is_py_space(c)
        || matches!(c, '｜' | '|' | ':' | '：' | ',' | '，' | '、' | ';' | '；' | '·' | '•')
}

/// 抹平排版差异的「内容骨架」，仅用于短消息精确判重与日志。
/// 刻意保留数字/文字/大小写：宁可漏掉一次近似去重，也不能永久漏发隔天日报。
pub fn content_key(content: &str) -> String {
    content.chars().filter(|c| !is_key_sep(*c)).collect()
}

/// 两段正文是不是「同一条消息的两种渲染」。
///
/// 判据是片段集合**完全相等**而非「重合度够高」：实测把日报一个数字改掉后
/// 相似度仍有 0.97，用阈值会把隔天日报吃掉，那是永久漏发，比多发一条严重。
pub fn is_same_message(a: &str, b: &str) -> bool {
    if content_key(a).chars().count() < NEAR_MIN_LEN
        || content_key(b).chars().count() < NEAR_MIN_LEN
    {
        return false;
    }
    let sa: HashSet<String> = content_pieces(a).into_iter().collect();
    let sb: HashSet<String> = content_pieces(b).into_iter().collect();
    !sa.is_empty() && sa == sb
}

#[cfg(test)]
mod tests {
    use super::*;

    // 期望值由现网 Python 实现算出（2026-11-07 对拍），不要手改。
    #[test]
    fn golden_md5_matches_python() {
        assert_eq!(
            fp_of("签收跟单专员 机器人", "今日订单 1200 单，已全部发出。"),
            "a32acfa5962b169ca12099104491f98a"
        );
        assert_eq!(
            fp_of("签收跟单专员", "今日订单 1200 单，已全部发出。"),
            "a32acfa5962b169ca12099104491f98a"
        );
        assert_eq!(
            fp_of("张三", "明早 8 点开始卸货，请提前安排月台。"),
            "60d7313109dbeed94e4a05dc0f16146b"
        );
    }

    #[test]
    fn robot_suffix_and_spaces_do_not_change_fingerprint() {
        assert_eq!(norm_sender("签收跟单专员 机器人"), "签收跟单专员");
        assert_eq!(norm_sender("签收\u{3000}跟单专员"), "签收跟单专员");
        assert_eq!(
            fp_of("签收跟单专员 机器人", "x"),
            fp_of("签收 跟单专员", "x")
        );
        // 不同机器人不能被混成同一个
        assert_ne!(fp_of("签收跟单专员", "x"), fp_of("拦截跟单专员", "x"));
    }

    #[test]
    fn short_messages_are_never_near_duplicates() {
        assert!(!is_same_message("收到", "收到"));
        assert!(!is_same_message("好的，马上安排", "好的 马上安排"));
    }

    #[test]
    fn reordered_card_layout_is_the_same_message() {
        let a = "指标 10/13 10/14 10/15 10/16 10/17 跟单任务数 30 21 23 20 17 拦截率 93.33%";
        let b = "跟单任务数 指标 10/17 10/16 10/15 10/14 10/13 17 20 23 21 30 93.33% 拦截率";
        assert!(is_same_message(a, b), "只换了排版顺序，应判为同一条消息");
    }

    #[test]
    fn changed_number_is_not_the_same_message() {
        let a = "指标 10/13 10/14 10/15 10/16 10/17 跟单任务数 30 21 23 20 17 拦截率 93.33%";
        let b = "指标 10/13 10/14 10/15 10/16 10/17 跟单任务数 30 21 23 20 17 拦截率 88.10%";
        assert!(!is_same_message(a, b), "数字变了必须判为不同消息，否则会永久漏发");
    }

    #[test]
    fn content_key_keeps_arrow_but_strips_other_separators() {
        assert_eq!(content_key("a→b"), "a→b");
        assert_eq!(content_key("a｜b"), "ab");
        assert_eq!(content_key("a，b"), "ab");
    }
}
