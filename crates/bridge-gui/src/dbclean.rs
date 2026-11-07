//! 消息数据库瘦身：与 Python `bridge/dbclean.py` 同口径的 Rust 实现。
//!
//! **安全边界（比 Python 版更严）**：Rust GUI 对 Python 正式版数据库永远只读，
//! 清理功能只在「独立版自带数据库」+「转发引擎已停止」时由界面放开按钮；
//! 本模块自身也再检查一次调用方传入的 `allow_write`，防止误用。
//!
//! 删：超龄 messages / deliveries + 孤儿投递记录；然后 checkpoint → VACUUM。
//! 不删：`data/attachments` 里的附件文件。

use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;
use std::time::Duration;

pub const MIN_KEEP_DAYS: i64 = 7;
pub const DEFAULT_KEEP_DAYS: i64 = 90;

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub keep_days: i64,
    pub file_bytes: u64,
    pub db_bytes: u64,
    pub message_rows: i64,
    pub message_old: i64,
    pub delivery_rows: i64,
    pub delivery_old: i64,
    pub delivery_orphan: i64,
    pub page_size: i64,
    pub page_count: i64,
    pub freelist_count: i64,
    pub cutoff: String,
    pub error: String,
}

impl Plan {
    pub fn doomed(&self) -> i64 {
        self.delivery_old + self.delivery_orphan
    }

    pub fn nothing_to_do(&self) -> bool {
        self.message_old == 0 && self.doomed() == 0 && self.freelist_count == 0
    }

    pub fn reclaimable(&self) -> u64 {
        let rows = self.message_rows + self.delivery_rows;
        let doomed = self.message_old + self.doomed();
        if self.db_bytes == 0 || rows <= 0 || doomed <= 0 {
            return 0;
        }
        (self.db_bytes as f64 * doomed as f64 / rows as f64) as u64
    }

    /// 设置中心预览框直接显示这几行。
    pub fn summary(&self) -> Vec<String> {
        let mut out = vec![
            format!(
                "数据库占盘 {}（主库 {}）",
                human_size(self.file_bytes),
                human_size(self.db_bytes)
            ),
            format!(
                "消息共 {} 条 · 超龄 {} 条{}",
                self.message_rows,
                self.message_old,
                if self.cutoff.is_empty() {
                    String::new()
                } else {
                    format!("（早于 {}）", self.cutoff)
                }
            ),
            format!(
                "投递记录共 {} 条 · 超龄 {} 条 · 孤儿 {} 条",
                self.delivery_rows, self.delivery_old, self.delivery_orphan
            ),
        ];
        if self.freelist_count > 0 {
            out.push(format!(
                "另有已删未回收的空白页 {}（VACUUM 会收回）",
                human_size((self.freelist_count * self.page_size) as u64)
            ));
        }
        if self.reclaimable() > 0 {
            out.push(format!("预计回收约 {}", human_size(self.reclaimable())));
        }
        out
    }
}

#[derive(Debug, Clone, Default)]
pub struct CleanupResult {
    pub deleted_messages: usize,
    pub deleted_deliveries: usize,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub vacuumed: bool,
    pub note: String,
    pub error: String,
}

impl CleanupResult {
    pub fn freed(&self) -> u64 {
        self.before_bytes.saturating_sub(self.after_bytes)
    }
}

pub fn human_size(n: u64) -> String {
    let mut v = n as f64;
    for unit in ["B", "KB", "MB", "GB"] {
        if v < 1024.0 || unit == "GB" {
            return if unit == "B" {
                format!("{v:.0} {unit}")
            } else {
                format!("{v:.1} {unit}")
            };
        }
        v /= 1024.0;
    }
    format!("{v:.1} GB")
}

fn file_bytes(p: &Path) -> u64 {
    let mut total = 0;
    for suffix in ["", "-wal", "-shm"] {
        let mut name = p.as_os_str().to_string_lossy().into_owned();
        name.push_str(suffix);
        if let Ok(m) = std::fs::metadata(&name) {
            total += m.len();
        }
    }
    total
}

fn connect(path: &Path, read_only: bool) -> Result<Connection, String> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    };
    let c = Connection::open_with_flags(path, flags)
        .map_err(|e| format!("打不开数据库：{e}"))?;
    c.busy_timeout(Duration::from_secs(8))
        .map_err(|e| format!("设置锁等待失败：{e}"))?;
    Ok(c)
}

/// 只读体检：算清会删多少行、能省多少空间，一行数据都不动。
pub fn scan(path: &Path, keep_days: i64) -> Result<Plan, String> {
    if keep_days < 0 || (keep_days > 0 && keep_days < MIN_KEEP_DAYS) {
        return Err(format!(
            "保留天数不能是负数，也不能少于 {MIN_KEEP_DAYS} 天（过短会影响去重与补发）"
        ));
    }
    let mut plan = Plan {
        keep_days,
        ..Default::default()
    };
    if !path.is_file() {
        plan.error = format!("数据库不存在：{}", path.display());
        return Ok(plan);
    }
    plan.file_bytes = file_bytes(path);
    plan.db_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let conn = match connect(path, true) {
        Ok(c) => c,
        Err(e) => {
            plan.error = e;
            return Ok(plan);
        }
    };
    let _ = conn.execute_batch("PRAGMA query_only = ON;");
    // 与 Python datetime.now() 同口径：SQLite 的 localtime 修饰符给本地墙上时间。
    plan.cutoff = if keep_days > 0 {
        conn.query_row(
            "SELECT datetime('now', 'localtime', ?1)",
            params![format!("-{keep_days} days")],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_default()
    } else {
        String::new()
    };

    let count = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> i64 {
        conn.query_row(sql, args, |r| r.get::<_, i64>(0)).unwrap_or(0)
    };
    plan.page_size = count("PRAGMA page_size", &[]);
    plan.page_count = count("PRAGMA page_count", &[]);
    plan.freelist_count = count("PRAGMA freelist_count", &[]);
    plan.message_rows = count("SELECT COUNT(*) FROM messages", &[]);
    plan.delivery_rows = count("SELECT COUNT(*) FROM deliveries", &[]);
    if !plan.cutoff.is_empty() {
        plan.message_old = count(
            "SELECT COUNT(*) FROM messages WHERE \
             COALESCE(NULLIF(TRIM(msg_time), ''), created_at) < ?1",
            &[&plan.cutoff],
        );
        plan.delivery_old = count(
            "SELECT COUNT(*) FROM deliveries WHERE created_at < ?1",
            &[&plan.cutoff],
        );
    }
    plan.delivery_orphan = count(
        "SELECT COUNT(*) FROM deliveries WHERE uid NOT IN (SELECT uid FROM messages)",
        &[],
    );
    Ok(plan)
}

/// 按体检结果动手。删行是一个事务；VACUUM 抢不到锁只降级提示，不把已删的事实报成失败。
pub fn execute(path: &Path, plan: &Plan, allow_write: bool) -> Result<CleanupResult, String> {
    if !allow_write {
        return Err("当前数据源不是本独立版自带的数据库，拒绝写入（正式版数据永远只读）".to_owned());
    }
    if !plan.error.is_empty() {
        return Err(plan.error.clone());
    }
    let mut res = CleanupResult {
        before_bytes: plan.file_bytes,
        ..Default::default()
    };
    let conn = connect(path, false)?;
    {
        let tx = conn.unchecked_transaction().map_err(|e| format!("开事务失败：{e}"))?;
        if !plan.cutoff.is_empty() {
            res.deleted_messages = tx
                .execute(
                    "DELETE FROM messages WHERE \
                     COALESCE(NULLIF(TRIM(msg_time), ''), created_at) < ?1",
                    params![plan.cutoff],
                )
                .map_err(|e| format!("删除旧消息失败（已回滚）：{e}"))?;
            res.deleted_deliveries = tx
                .execute(
                    "DELETE FROM deliveries WHERE created_at < ?1",
                    params![plan.cutoff],
                )
                .map_err(|e| format!("删除旧投递记录失败（已回滚）：{e}"))?;
        }
        res.deleted_deliveries += tx
            .execute(
                "DELETE FROM deliveries WHERE uid NOT IN (SELECT uid FROM messages)",
                [],
            )
            .map_err(|e| format!("删除孤儿记录失败（已回滚）：{e}"))?;
        tx.commit().map_err(|e| format!("提交失败：{e}"))?;
    }
    // WAL 里还挂着页时 VACUUM 会 busy，先 checkpoint 是硬要求。
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    match conn.execute_batch("VACUUM;") {
        Ok(()) => {
            res.vacuumed = true;
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
        Err(e) => {
            res.note = format!(
                "数据已删除，但空间没回收（{e}）——多半是转发程序还在占用库，停掉后再点一次即可。"
            );
        }
    }
    drop(conn);
    res.after_bytes = file_bytes(path);
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mm_dbclean_{}_{}", std::process::id(), name));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("bridge.db");
        let _ = std::fs::remove_file(&p);
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            "CREATE TABLE messages (uid TEXT PRIMARY KEY, msg_time TEXT, created_at TEXT, content TEXT);
             CREATE TABLE deliveries (id INTEGER PRIMARY KEY, uid TEXT, status TEXT, created_at TEXT);
             INSERT INTO messages VALUES
               ('old','2020-01-01 10:00:00','2020-01-01 10:00:01','旧消息'),
               ('new','2099-01-01 10:00:00','2099-01-01 10:00:01','新消息');
             INSERT INTO deliveries VALUES
               (1,'old','ok','2020-01-01 10:00:02'),
               (2,'new','ok','2099-01-01 10:00:02'),
               (3,'ghost','fail','2020-01-01 10:00:03');",
        )
        .unwrap();
        drop(c);
        p
    }

    #[test]
    fn scan_counts_old_rows_and_orphans() {
        let p = fixture("scan");
        let plan = scan(&p, 30).unwrap();
        assert!(plan.error.is_empty(), "{}", plan.error);
        assert_eq!(plan.message_rows, 2);
        assert_eq!(plan.message_old, 1);
        assert_eq!(plan.delivery_old, 2); // 一条超龄消息 + 一条超龄孤儿
        assert_eq!(plan.delivery_orphan, 1);
        assert!(!plan.cutoff.is_empty());
        assert!(plan.summary().len() >= 3);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn keep_zero_only_removes_orphans() {
        let p = fixture("zero");
        let plan = scan(&p, 0).unwrap();
        assert!(plan.cutoff.is_empty());
        assert_eq!(plan.message_old, 0);
        assert_eq!(plan.delivery_old, 0);
        assert_eq!(plan.delivery_orphan, 1);
        let r = execute(&p, &plan, true).unwrap();
        assert_eq!(r.deleted_messages, 0);
        assert_eq!(r.deleted_deliveries, 1);
        assert!(r.vacuumed);
        let c = Connection::open(&p).unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(c);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn rejects_short_keep_and_readonly_db() {
        let p = fixture("guard");
        assert!(scan(&p, 3).is_err());
        assert!(scan(&p, -1).is_err());
        let plan = scan(&p, 30).unwrap();
        assert!(execute(&p, &plan, false).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}
