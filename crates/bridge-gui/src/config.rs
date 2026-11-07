//! `config.yaml` 轻量读写层。
//!
//! 为什么不引 `serde_yaml`：本程序的编辑目标是**用户手写的带大量中文注释**的
//! 配置文件，序列化回去会把注释、空行、行尾说明全部抹掉。这里采用「按行定位 +
//! 原样替换值」的做法，只动目标键那一行，其它字节保持不变（与安装器注入
//! `gateway_token` 的策略一致）。
//!
//! 边界：只支持两级结构（顶层 section + 缩进 key），以及 `routes:` 下的 `- name:`
//! 列表展示；复杂嵌套请到「设置中心 → 原文编辑」里改。保存前自动留一份 `.bak`。

use std::fs;
use std::path::{Path, PathBuf};

use crate::datasource;

/// 一次拿给界面展示的配置摘要（全部是标量，缺键给空串，不 panic）。
#[derive(Debug, Clone, Default)]
pub struct ConfigSummary {
    pub mode: String,
    pub time_filter: String,
    pub chat_names: String,
    pub companies: String,
    pub gateway_host: String,
    pub gateway_port: String,
    pub gateway_token: String,
    pub android_parse: String,
    pub attach_enabled: String,
    pub driver: String,
    pub dry_run: String,
    pub interval: String,
    pub max_backlog_min: String,
    pub keep_days: String,
    pub notify_enabled: String,
    pub reconcile_enabled: String,
}

#[derive(Debug, Clone, Default)]
pub struct RouteView {
    pub name: String,
    pub companies: String,
    pub keywords: String,
    pub targets: String,
    pub template: String,
}

#[derive(Debug, Clone)]
pub struct ConfigDoc {
    pub path: PathBuf,
    pub text: String,
    pub error: Option<String>,
}

impl ConfigDoc {
    pub fn load() -> Self {
        let path = datasource::config_path();
        let mut d = Self {
            path,
            text: String::new(),
            error: None,
        };
        d.reload();
        d
    }

    pub fn reload(&mut self) {
        match fs::read_to_string(&self.path) {
            Ok(t) => {
                self.text = t;
                self.error = None;
            }
            Err(e) => {
                self.error = Some(format!("读取配置失败：{e}"));
                if self.text.is_empty() {
                    self.text = String::new();
                }
            }
        }
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    /// 取 `section.key` 的标量值；忽略注释、引号、块列表的 `- ` 子项。
    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        let lines = self.lines();
        let range = section_range(&lines, section)?;
        for line in &lines[range] {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
                continue;
            }
            let Some((k, v)) = split_kv(trimmed) else {
                continue;
            };
            if k == key {
                return Some(parse_scalar(v));
            }
        }
        None
    }

    /// 取列表键的值：既认 `key: ["a", "b"]`，也认「`key:` + 下面若干 `- "a"`」。
    pub fn get_list(&self, section: &str, key: &str) -> String {
        let lines = self.lines();
        let Some(range) = section_range(&lines, section) else {
            return String::new();
        };
        let base_indent;
        for (i, line) in lines[range.clone()].iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
                continue;
            }
            let Some((k, v)) = split_kv(trimmed) else {
                continue;
            };
            if k != key {
                continue;
            }
            base_indent = indent_of(line);
            if !parse_scalar(v).is_empty() {
                return normalize_list(&parse_scalar(v));
            }
            let mut items = Vec::new();
            for next in &lines[range.start + i + 1..range.end] {
                let nt = next.trim();
                if nt.is_empty() || nt.starts_with('#') {
                    continue;
                }
                if indent_of(next) <= base_indent || !nt.starts_with('-') {
                    break;
                }
                items.push(parse_scalar(nt.trim_start_matches('-').trim()));
            }
            return items.join(", ");
        }
        String::new()
    }

    /// 替换 `section.key` 这一行的值，保留缩进、键名与行尾注释。
    /// 若原来是多行块列表，会把紧随其后的 `- ` 子项一并收进新值里。
    /// 返回是否真的找到了目标键。
    pub fn set(&mut self, section: &str, key: &str, value: &str) -> bool {
        let mut lines = self.lines();
        let Some(range) = section_range(&lines, section) else {
            return false;
        };
        let mut target = None;
        for (i, line) in lines[range.clone()].iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
                continue;
            }
            if let Some((k, _)) = split_kv(trimmed) {
                if k == key {
                    target = Some(range.start + i);
                    break;
                }
            }
        }
        let Some(i) = target else {
            return false;
        };
        let base_indent = indent_of(&lines[i]);
        // 吃掉紧随其后的块列表子项；注释 / 空行一律不动（用户写的说明要留着）。
        let mut end = i + 1;
        while end < lines.len() && end < range.end {
            let nt = lines[end].trim();
            if nt.starts_with('-') && indent_of(&lines[end]) > base_indent {
                end += 1;
            } else {
                break;
            }
        }
        let rendered = render_value(value);
        let comment = tail_comment(&lines[i]);
        let indent = " ".repeat(base_indent);
        let new_line = format!("{indent}{key}: {rendered}{comment}");
        lines.splice(i..end, std::iter::once(new_line));
        self.text = join_lines(&lines, self.text.contains("\r\n"));
        true
    }

    /// 保存当前文本：先备份 `.bak`，再写临时文件并原子替换。
    pub fn save(&mut self) -> Result<String, String> {
        if self.path.as_os_str().is_empty() {
            return Err("没有可写的配置文件路径".to_owned());
        }
        let orig = fs::read(&self.path).unwrap_or_default();
        let bak = with_suffix(&self.path, ".bak");
        if !orig.is_empty() {
            fs::write(&bak, &orig).map_err(|e| format!("备份失败：{e}"))?;
        }
        // 直接覆写：安装目录在 Program Files 时可能无写权限，失败要如实报出来，
        // 不假装保存成功。备份已经先落盘，出错也能手工还原。
        fs::write(&self.path, self.text.as_bytes())
            .map_err(|e| format!("写入配置失败（可能需要以管理员身份运行）：{e}"))?;
        Ok(format!("已保存到 {}（原文件备份为 {}）", self.path.display(), bak.display()))
    }

    /// 用界面表单覆盖一批键，返回成功写入的键数。
    pub fn set_many(&mut self, edits: &[(&str, &str, &str)]) -> usize {
        let mut n = 0;
        for (section, key, value) in edits {
            if self.set(section, key, value) {
                n += 1;
            }
        }
        n
    }

    pub fn summary(&self) -> ConfigSummary {
        let g = |s: &str, k: &str| self.get(s, k).unwrap_or_default();
        ConfigSummary {
            mode: g("dingtalk", "mode"),
            time_filter: g("dingtalk", "time_filter"),
            chat_names: self.get_list("dingtalk", "chat_names"),
            companies: self.get_list("dingtalk", "companies"),
            gateway_host: g("dingtalk", "gateway_host"),
            gateway_port: g("dingtalk", "gateway_port"),
            gateway_token: g("dingtalk", "gateway_token"),
            android_parse: g("dingtalk", "android_parse"),
            attach_enabled: g("dingtalk", "attach_enabled"),
            driver: g("wechat", "driver"),
            dry_run: g("wechat", "dry_run"),
            interval: g("wechat", "interval"),
            max_backlog_min: g("wechat", "max_backlog_min"),
            keep_days: g("storage", "keep_days"),
            notify_enabled: g("notify", "enabled"),
            reconcile_enabled: g("reconcile", "enabled"),
        }
    }

    /// `routes:` 下的规则列表（只读展示，避免误改嵌套结构）。
    pub fn routes(&self) -> Vec<RouteView> {
        let lines = self.lines();
        let Some(range) = section_range(&lines, "routes") else {
            return Vec::new();
        };
        let mut out: Vec<RouteView> = Vec::new();
        let mut last_list_key = String::new();
        for line in &lines[range] {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("- ") {
                if let Some((k, v)) = split_kv(rest) {
                    if k == "name" {
                        out.push(RouteView {
                            name: parse_scalar(v),
                            ..Default::default()
                        });
                        last_list_key.clear();
                        continue;
                    }
                }
                // `targets:` / `companies:` 下另起一行写的 `- "值"`。
                let item = parse_scalar(rest);
                if let Some(cur) = out.last_mut() {
                    if !item.is_empty() {
                        let slot = match last_list_key.as_str() {
                            "companies" => &mut cur.companies,
                            "keywords" => &mut cur.keywords,
                            "targets" => &mut cur.targets,
                            _ => continue,
                        };
                        if !slot.is_empty() {
                            slot.push_str(", ");
                        }
                        slot.push_str(&item);
                    }
                }
                continue;
            }
            let Some(cur) = out.last_mut() else {
                continue;
            };
            if let Some((k, v)) = split_kv(trimmed) {
                let value = parse_scalar(v);
                last_list_key = match k {
                    "companies" | "keywords" | "targets" => k.to_owned(),
                    _ => String::new(),
                };
                match k {
                    "companies" => cur.companies = normalize_list(&value),
                    "keywords" => cur.keywords = normalize_list(&value),
                    "targets" => cur.targets = normalize_list(&value),
                    "template" => cur.template = value,
                    _ => {}
                }
            }
        }
        out
    }

    fn lines(&self) -> Vec<String> {
        self.text
            .lines()
            .map(|l| l.trim_end_matches('\r').to_owned())
            .collect()
    }
}

/// 规则页的可编辑表单。只收标量键，列表键（群名 / 公司）用 `set` 的块列表折叠能力。
#[derive(Debug, Clone)]
pub struct RuleForm {
    pub mode: String,
    pub time_filter: String,
    pub chat_names: String,
    pub companies: String,
    pub gateway_port: String,
    pub android_parse: bool,
    pub attach_enabled: bool,
    pub dry_run: bool,
    pub driver: String,
    pub interval: String,
    pub max_backlog_min: String,
    pub keep_days: String,
}

impl RuleForm {
    pub fn from_doc(doc: &ConfigDoc) -> Self {
        let s = doc.summary();
        Self {
            mode: s.mode,
            time_filter: s.time_filter,
            chat_names: s.chat_names,
            companies: s.companies,
            gateway_port: s.gateway_port,
            android_parse: truthy(&s.android_parse),
            attach_enabled: truthy(&s.attach_enabled),
            dry_run: truthy(&s.dry_run),
            driver: s.driver,
            interval: s.interval,
            max_backlog_min: s.max_backlog_min,
            keep_days: s.keep_days,
        }
    }

    /// 把表单写回文档；列表字段用逗号分隔，写成 YAML 行内列表。
    pub fn apply(&self, doc: &mut ConfigDoc) -> usize {
        let list = |s: &str| -> String {
            let items: Vec<String> = s
                .split([',', '，', ';', '；'])
                .map(|x| x.trim().trim_matches(['"', '\'']).to_owned())
                .filter(|x| !x.is_empty())
                .map(|x| format!("\"{}\"", x.replace('"', "\\\"")))
                .collect();
            if items.is_empty() {
                "[]".to_owned()
            } else {
                format!("[{}]", items.join(", "))
            }
        };
        let edits: Vec<(&str, &str, String)> = vec![
            ("dingtalk", "mode", self.mode.clone()),
            ("dingtalk", "time_filter", self.time_filter.clone()),
            ("dingtalk", "chat_names", list(&self.chat_names)),
            ("dingtalk", "companies", list(&self.companies)),
            ("dingtalk", "gateway_port", self.gateway_port.clone()),
            ("dingtalk", "android_parse", self.android_parse.to_string()),
            ("dingtalk", "attach_enabled", self.attach_enabled.to_string()),
            ("wechat", "dry_run", self.dry_run.to_string()),
            ("wechat", "driver", self.driver.clone()),
            ("wechat", "interval", self.interval.clone()),
            ("wechat", "max_backlog_min", self.max_backlog_min.clone()),
            ("storage", "keep_days", self.keep_days.clone()),
        ];
        let refs: Vec<(&str, &str, &str)> = edits
            .iter()
            .map(|(s, k, v)| (*s, *k, v.as_str()))
            .collect();
        doc.set_many(&refs)
    }
}

pub fn truthy(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1")
}

/// 顶层 section 的行范围 `[start, end)`；end 是下一个无缩进键或文件末尾。
fn section_range(lines: &[String], section: &str) -> Option<std::ops::Range<usize>> {
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        if indent_of(line) > 0 {
            continue;
        }
        let key = trimmed.split(':').next().unwrap_or("").trim();
        if start.is_some() {
            return Some(start.unwrap()..i);
        }
        if key == section {
            start = Some(i + 1);
        }
    }
    start.map(|s| s..lines.len())
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn split_kv(s: &str) -> Option<(&str, &str)> {
    let pos = s.find(':')?;
    Some((s[..pos].trim(), &s[pos + 1..]))
}

/// 去掉行尾注释（`#` 前有空白且不在引号内），再剥掉成对引号。
fn parse_scalar(raw: &str) -> String {
    let mut in_quote = false;
    let mut cut = raw.len();
    let bytes = raw.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b'"' | b'\'' => in_quote = !in_quote,
            b'#' if !in_quote && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                cut = i;
                break;
            }
            _ => {}
        }
    }
    let v = raw[..cut].trim();
    let v = if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        &v[1..v.len() - 1]
    } else {
        v
    };
    v.trim().to_owned()
}

/// 行尾注释（含前导空白），set() 时原样拼回。
fn tail_comment(line: &str) -> String {
    let Some((_, v)) = split_kv(line.trim()) else {
        return String::new();
    };
    let mut in_quote = false;
    let bytes = v.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b'"' | b'\'' => in_quote = !in_quote,
            b'#' if !in_quote && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                // 连注释前的空白一起保留，替换后视觉排版不跳格。
                let mut start = i;
                while start > 0 && bytes[start - 1].is_ascii_whitespace() {
                    start -= 1;
                }
                return v[start..].trim_end().to_owned();
            }
            _ => {}
        }
    }
    String::new()
}

/// bool / 数字 / 行内列表 / 空值不加引号，其余按 YAML 双引号字符串输出。
fn render_value(value: &str) -> String {
    let v = value.trim();
    if v.is_empty()
        || matches!(v, "true" | "false" | "True" | "False" | "null" | "~")
        || v.parse::<f64>().is_ok()
        || (v.starts_with('[') && v.ends_with(']'))
    {
        v.to_owned()
    } else {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn normalize_list(v: &str) -> String {
    let s = v.trim();
    if s.starts_with('[') && s.ends_with(']') {
        return s[1..s.len() - 1]
            .split(',')
            .map(|x| x.trim().trim_matches(['"', '\'']).to_owned())
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
    }
    s.trim_matches(['"', '\'']).to_owned()
}

fn join_lines(lines: &[String], crlf: bool) -> String {
    let sep = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(sep);
    out.push_str(sep);
    out
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_string_lossy().into_owned();
    s.push_str(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> ConfigDoc {
        ConfigDoc {
            path: PathBuf::new(),
            text: concat!(
                "# 头部注释\n",
                "storage:\n",
                "  db_path: \"data/bridge.db\"\n",
                "  keep_days: 90\n",
                "dingtalk:\n",
                "  mode: \"ocr\"  # 采集模式\n",
                "  time_filter: \"today\"\n",
                "  companies:\n",
                "    - \"示例科技有限公司\"\n",
                "  chat_names: []\n",
                "  gateway_port: 8765\n",
                "  gateway_token: \"\"\n",
                "  android_parse: false\n",
                "  attach_enabled: true\n",
                "wechat:\n",
                "  dry_run: true\n",
                "  driver: \"auto\"\n",
                "  interval: 5.0\n",
                "  max_backlog_min: 30\n",
                "routes:\n",
                "  - name: \"示例公司全部消息\"\n",
                "    companies: [\"示例科技有限公司\"]\n",
                "    keywords: []\n",
                "    targets:\n",
                "      - \"测试群\"\n",
                "    template: \"【{company}】{sender}\\n{content}\"\n"
            )
            .to_owned(),
            error: None,
        }
    }

    #[test]
    fn get_reads_scalars_without_comments_or_quotes() {
        let d = doc();
        assert_eq!(d.get("storage", "keep_days").as_deref(), Some("90"));
        assert_eq!(d.get("dingtalk", "mode").as_deref(), Some("ocr"));
        assert_eq!(d.get("dingtalk", "gateway_token").as_deref(), Some(""));
        assert_eq!(d.get("wechat", "dry_run").as_deref(), Some("true"));
        assert_eq!(d.get("dingtalk", "nope"), None);
        assert_eq!(d.get("nope", "mode"), None);
    }

    #[test]
    fn get_list_handles_inline_and_block_forms() {
        let d = doc();
        assert_eq!(d.get_list("dingtalk", "companies"), "示例科技有限公司");
        assert_eq!(d.get_list("dingtalk", "chat_names"), "");
    }

    #[test]
    fn set_keeps_indentation_and_trailing_comment() {
        let mut d = doc();
        assert!(d.set("dingtalk", "mode", "android"));
        assert!(d.text.contains("  mode: \"android\"  # 采集模式\n"));
        assert_eq!(d.get("dingtalk", "mode").as_deref(), Some("android"));
        // 注释、其它节、头部注释都还在
        assert!(d.text.starts_with("# 头部注释\n"));
        assert!(d.text.contains("  keep_days: 90\n"));
        assert!(!d.set("missing", "key", "x"));
    }

    #[test]
    fn set_folds_block_list_into_inline_list() {
        let mut d = doc();
        assert!(d.set("dingtalk", "companies", "[\"A公司\", \"B公司\"]"));
        assert!(d.text.contains("  companies: [\"A公司\", \"B公司\"]\n"));
        assert!(!d.text.contains("    - \"示例科技有限公司\""));
        assert_eq!(d.get_list("dingtalk", "companies"), "A公司, B公司");
    }

    #[test]
    fn render_value_only_quotes_strings() {
        assert_eq!(render_value("false"), "false");
        assert_eq!(render_value("5.0"), "5.0");
        assert_eq!(render_value("[]"), "[]");
        assert_eq!(render_value("ocr"), "\"ocr\"");
        assert_eq!(render_value("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn routes_are_parsed_for_display() {
        let r = doc().routes();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "示例公司全部消息");
        assert_eq!(r[0].companies, "示例科技有限公司");
        assert_eq!(r[0].targets, "测试群"); // 块列表写法也要认
        assert!(r[0].template.contains("{sender}"));
    }

    #[test]
    fn set_keeps_comments_between_list_and_next_key() {
        let mut d = doc();
        d.text = d
            .text
            .replace("  chat_names: []", "  # 会话白名单说明\n  chat_names: []");
        assert!(d.set("dingtalk", "companies", "[\"A\"]"));
        assert!(d.text.contains("# 会话白名单说明"));
        assert!(!d.text.contains("    - \"示例科技有限公司\""));
    }

    #[test]
    fn form_writes_scalars_back() {
        let mut d = doc();
        let mut f = RuleForm::from_doc(&d);
        assert!(f.android_parse == false);
        f.mode = "android".to_owned();
        f.chat_names = "群A, 群B".to_owned();
        f.dry_run = false;
        f.keep_days = "30".to_owned();
        assert_eq!(f.apply(&mut d), 12);
        assert_eq!(d.get("dingtalk", "mode").as_deref(), Some("android"));
        assert_eq!(d.get_list("dingtalk", "chat_names"), "群A, 群B");
        assert_eq!(d.get("wechat", "dry_run").as_deref(), Some("false"));
        assert_eq!(d.get("storage", "keep_days").as_deref(), Some("30"));
    }

    #[test]
    fn save_writes_backup_and_content() {
        let dir = std::env::temp_dir().join(format!("mm_cfg_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let p = dir.join("config.yaml");
        fs::write(&p, "old: 1\n").unwrap();
        let mut d = ConfigDoc {
            path: p.clone(),
            text: "new: 2\n".to_owned(),
            error: None,
        };
        d.save().unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new: 2\n");
        assert_eq!(fs::read_to_string(dir.join("config.yaml.bak")).unwrap(), "old: 1\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
