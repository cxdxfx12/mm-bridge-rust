//! 只读数据层：读取 Python 正式版 `bridge.db` 的真实消息与投递记录。
//!
//! 迁移约定（方案 A / 灰度期）：
//! - **只读**：连接由适配层用 `SQLITE_OPEN_READ_ONLY` + `PRAGMA query_only=ON` 打开，
//!   本模块只执行 SELECT，绝不写库、绝不建表、绝不抢占生产连接。
//! - **口径对齐 Python**：`bridge/storage.py::stats()` 统计累计 messages 与
//!   deliveries 按 status 分组；这里用完全相同的 COUNT / GROUP BY，数字两边必须对得上。
//! - 系统相关的路径发现、本地时间、窗口检测不进本 crate，留在 `bridge-gui::datasource`。

use rusqlite::{params, Connection, OptionalExtension};

/// 与 Python `stats()` 对齐的统计口径，外加界面需要的派生值。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Kpi {
    pub total_messages: i64,
    pub total_ok: i64,
    pub total_fail: i64,
    /// 今日成功投递数（`created_at` 前 10 位等于本地日期）。
    pub today_ok: i64,
    /// 累计投递成功率，百分比（ok / (ok+fail) * 100），无投递时为 0。
    pub success_rate: f64,
    pub last_message_at: Option<String>,
    pub last_delivery_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlowRow {
    /// HH:MM:SS，取自 messages.created_at。
    pub time: String,
    pub sender: String,
    pub summary: String,
    /// "已投递" / "投递失败" / "待投递"
    pub status: String,
    pub ok: bool,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub time: String,
    pub ok: bool,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// 实际读到的数据库路径；None 表示没找到正式版数据。
    pub db_path: Option<String>,
    /// 可读的错误说明（找不到库 / 读失败），界面直接显示。
    pub error: Option<String>,
    /// Python 正式版 GUI 窗口是否开着。
    pub official_gui_open: bool,
    /// 正式版主程序路径，供「打开正式版」按钮使用。
    pub official_exe: Option<String>,
    pub chat_name: String,
    /// 本次刷新时间 HH:MM:SS。
    pub refreshed_at: String,
    pub kpi: Kpi,
    pub flow: Vec<FlowRow>,
    pub entries: Vec<LogEntry>,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.error.is_some() || self.kpi.total_messages == 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadResult {
    pub kpi: Kpi,
    pub flow: Vec<FlowRow>,
    pub entries: Vec<LogEntry>,
    pub chat_name: String,
}

/// 从只读连接一次性读出界面需要的全部数据。
pub fn read_all(conn: &Connection, today: &str) -> rusqlite::Result<ReadResult> {
    Ok(ReadResult {
        kpi: read_kpi(conn, today)?,
        flow: read_flow(conn)?,
        entries: read_entries(conn)?,
        chat_name: read_chat_name(conn)?,
    })
}

fn read_kpi(conn: &Connection, today: &str) -> rusqlite::Result<Kpi> {
    let total_messages: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;

    let mut total_ok = 0_i64;
    let mut total_fail = 0_i64;
    {
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM deliveries GROUP BY status")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (status, n) = row?;
            match status.as_str() {
                "ok" => total_ok = n,
                "fail" => total_fail = n,
                _ => {}
            }
        }
    }

    let today_ok: i64 = conn.query_row(
        "SELECT COUNT(*) FROM deliveries WHERE status = 'ok' AND substr(created_at, 1, 10) = ?1",
        params![today],
        |r| r.get(0),
    )?;

    let last_message_at: Option<String> =
        conn.query_row("SELECT MAX(created_at) FROM messages", [], |r| r.get(0))?;
    let last_delivery_at: Option<String> =
        conn.query_row("SELECT MAX(created_at) FROM deliveries", [], |r| r.get(0))?;

    let total = total_ok + total_fail;
    let success_rate = if total > 0 {
        total_ok as f64 * 100.0 / total as f64
    } else {
        0.0
    };

    Ok(Kpi {
        total_messages,
        total_ok,
        total_fail,
        today_ok,
        success_rate,
        last_message_at,
        last_delivery_at,
    })
}

/// 每条消息取「最后一次投递」的状态（deliveries 里可能有重试），最新的排在最前。
fn read_flow(conn: &Connection) -> rusqlite::Result<Vec<FlowRow>> {
    let mut stmt = conn.prepare(
        "SELECT m.msg_time, COALESCE(m.sender_name, ''), COALESCE(m.content, ''), \
                COALESCE(m.msg_type, ''), COALESCE(d.status, ''), \
                COALESCE(d.target, ''), m.created_at \
         FROM messages m \
         LEFT JOIN deliveries d \
           ON d.id = (SELECT MAX(id) FROM deliveries WHERE uid = m.uid) \
         ORDER BY m.created_at DESC LIMIT 8",
    )?;
    let rows = stmt.query_map([], |r| {
        let msg_time: String = r.get(0)?;
        let sender: String = r.get(1)?;
        let content: String = r.get(2)?;
        let msg_type: String = r.get(3)?;
        let status: String = r.get(4)?;
        let target: String = r.get(5)?;
        let created_at: String = r.get(6)?;
        Ok((msg_time, sender, content, msg_type, status, target, created_at))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (msg_time, sender, content, msg_type, status, target, created_at) = row?;
        let (label, ok) = match status.as_str() {
            "ok" => ("已投递", true),
            "fail" => ("投递失败", false),
            _ => ("待投递", false),
        };
        out.push(FlowRow {
            time: hhmmss(if msg_time.is_empty() {
                &created_at
            } else {
                &msg_time
            }),
            sender: if sender.trim().is_empty() {
                "—".to_owned()
            } else {
                clean(&sender)
            },
            summary: summarize(&content, &msg_type, 28),
            status: label.to_owned(),
            ok,
            target: clean(&target),
        });
    }
    Ok(out)
}

fn read_entries(conn: &Connection) -> rusqlite::Result<Vec<LogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT created_at, status, target, COALESCE(error, '') \
         FROM deliveries ORDER BY id DESC LIMIT 8",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (created_at, status, target, error) = row?;
        let ok = status == "ok";
        let text = if ok {
            format!("已投递到「{}」{}", clean(&target), clean(&error))
        } else {
            let e = clean(&error);
            if e.is_empty() {
                format!("投递失败：{}", clean(&target))
            } else {
                shorten(&e, 52)
            }
        };
        out.push(LogEntry {
            time: hhmmss(&created_at),
            ok,
            text,
        });
    }
    out.reverse(); // 界面按时间正序显示
    Ok(out)
}

fn read_chat_name(conn: &Connection) -> rusqlite::Result<String> {
    Ok(conn
        .query_row(
            "SELECT chat_name FROM messages \
             WHERE chat_name IS NOT NULL AND chat_name <> '' \
             ORDER BY created_at DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .unwrap_or_default())
}

/// "YYYY-MM-DD HH:MM:SS" → "HH:MM:SS"，取不到就原样返回。
pub fn hhmmss(s: &str) -> String {
    s.get(11..19).unwrap_or(s).to_owned()
}

/// "YYYY-MM-DD HH:MM:SS" → "YYYY-MM-DD"。
pub fn date_of(s: &str) -> String {
    s.get(..10).unwrap_or(s).to_owned()
}

/// 去掉控制字符与 Unicode 私有区图标（钉钉正文里常见的 \u{f218} 之类），避免显示豆腐块。
pub fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !('\u{e000}'..='\u{f8ff}').contains(c))
        .collect::<String>()
        .replace(['\n', '\r', '\t'], " ")
        .trim()
        .to_owned()
}

pub fn shorten(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_owned();
    }
    format!("{}…", chars[..max].iter().collect::<String>())
}

pub fn summarize(content: &str, msg_type: &str, max: usize) -> String {
    let first = content
        .lines()
        .find(|l| !clean(l).is_empty())
        .unwrap_or("")
        .to_owned();
    let base = if msg_type == "file" {
        format!("[文件] {}", clean(&first))
    } else {
        clean(&first)
    };
    let s = if base.trim().is_empty() {
        "(无文本内容)".to_owned()
    } else {
        base
    };
    shorten(&s, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE messages (
                uid TEXT PRIMARY KEY, source TEXT NOT NULL, chat_id TEXT, chat_name TEXT,
                sender_id TEXT, sender_name TEXT, company TEXT, content TEXT,
                msg_type TEXT, media_path TEXT, msg_time TEXT, raw_id TEXT,
                created_at TEXT NOT NULL);
             CREATE TABLE deliveries (
                id INTEGER PRIMARY KEY AUTOINCREMENT, uid TEXT NOT NULL, target TEXT NOT NULL,
                status TEXT NOT NULL, error TEXT, created_at TEXT NOT NULL);
             INSERT INTO messages VALUES
               ('u1','android','','【数字质控】大江东公司','','签收跟单专员','大江东','今日订单 1200 单','text','','2026-11-06 16:13:31','','2026-11-06 16:13:35'),
               ('u2','android','','【数字质控】大江东公司','','服务诊断专员','大江东','表格\n姓名 单号','text','','2026-11-07 09:00:00','','2026-11-07 09:00:02'),
               ('u3','dingtalk','','','','','','','file','a.xlsx','2026-11-07 09:10:00','','2026-11-07 09:10:05');
             INSERT INTO deliveries (uid,target,status,error,created_at) VALUES
               ('u1','群A','fail','第一次失败','2026-11-06 16:13:36'),
               ('u1','群A','ok','已发送（检测到发出气泡）','2026-11-06 16:14:02'),
               ('u2','群A','ok','','2026-11-07 09:00:20'),
               ('u3','群A','fail','发送文件后未检测到文件卡片','2026-11-07 09:10:30');",
        )
        .unwrap();
        c
    }

    #[test]
    fn kpi_matches_python_stats_semantics() {
        let c = fixture();
        let k = read_kpi(&c, "2026-11-07").unwrap();
        assert_eq!(k.total_messages, 3);
        assert_eq!(k.total_ok, 2);
        assert_eq!(k.total_fail, 2);
        assert_eq!(k.today_ok, 1);
        assert!((k.success_rate - 50.0).abs() < 1e-9);
        assert_eq!(k.last_message_at.as_deref(), Some("2026-11-07 09:10:05"));
    }

    #[test]
    fn flow_takes_latest_delivery_per_uid() {
        let c = fixture();
        let f = read_flow(&c).unwrap();
        assert_eq!(f.len(), 3);
        // 最新的消息排最前
        assert_eq!(f[0].time, "09:10:00");
        assert_eq!(f[0].status, "投递失败");
        assert!(f[0].summary.starts_with("[文件]"));
        // u1 有两条投递（先 fail 后 ok），流水必须显示最后一次的 ok
        let u1 = f.iter().find(|r| r.time == "16:13:31").unwrap();
        assert_eq!(u1.status, "已投递");
        assert!(u1.ok);
    }

    #[test]
    fn entries_are_time_ordered_and_readable() {
        let c = fixture();
        let e = read_entries(&c).unwrap();
        assert_eq!(e.len(), 4);
        assert_eq!(e[0].time, "16:13:36");
        assert!(!e[0].ok);
        assert_eq!(e.last().unwrap().time, "09:10:30");
        assert!(e[1].ok);
        assert!(e[1].text.contains("已投递到"));
    }

    #[test]
    fn private_use_glyphs_are_stripped() {
        assert_eq!(clean("姓名\u{f218}\u{f3b5}单号"), "姓名单号");
        assert_eq!(hhmmss("2026-11-06 16:13:41"), "16:13:41");
        assert_eq!(date_of("2026-11-06 16:13:41"), "2026-11-06");
        assert_eq!(summarize("第一行\n第二行", "text", 10), "第一行");
        assert_eq!(shorten("一二三四五", 3), "一二三…");
    }
}
