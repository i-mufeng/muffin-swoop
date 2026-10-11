//! SQLite 持久化：任务表 + 设置键值表。
//!
//! 引擎在任务新增/状态变化时立即写入，下载中每秒刷新一次区段表。
//! 所有调用都是同步的，内部用 Mutex 串行化；单次写入在毫秒级。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;
use crate::segment::SegmentMap;
use crate::task::{TaskId, TaskStatus};

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS tasks (
    id           INTEGER PRIMARY KEY,
    url          TEXT    NOT NULL,
    dir          TEXT    NOT NULL,
    file_name    TEXT    NOT NULL DEFAULT '',
    user_named   INTEGER NOT NULL DEFAULT 0,
    total        INTEGER,
    resumable    INTEGER NOT NULL DEFAULT 0,
    etag         TEXT,
    status       TEXT    NOT NULL,
    error        TEXT,
    segments     TEXT    NOT NULL DEFAULT '',
    downloaded   INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    completed_at INTEGER,
    headers      TEXT    NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// 建表之后追加的列：旧库启动时按需 `ALTER TABLE` 补上（幂等）
const ADDED_TASK_COLUMNS: [(&str, &str); 1] = [("headers", "TEXT NOT NULL DEFAULT '[]'")];

fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    let existing: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('tasks')")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for (name, decl) in ADDED_TASK_COLUMNS {
        if !existing.iter().any(|c| c == name) {
            conn.execute_batch(&format!("ALTER TABLE tasks ADD COLUMN {name} {decl}"))?;
        }
    }
    Ok(())
}

/// 任务在数据库里的完整形态
#[derive(Clone, Debug)]
pub struct TaskRecord {
    /// 0 表示尚未分配（插入时由数据库生成）
    pub id: TaskId,
    pub url: String,
    pub dir: PathBuf,
    pub file_name: String,
    pub user_named: bool,
    pub total: Option<u64>,
    pub resumable: bool,
    pub etag: Option<String>,
    pub status: TaskStatus,
    pub segments: SegmentMap,
    pub downloaded: u64,
    /// unix 秒
    pub created_at: i64,
    pub completed_at: Option<i64>,
    /// 每个请求附加的头（浏览器带来的 Cookie / Referer / User-Agent），明文存储
    pub headers: Vec<(String, String)>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        init(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// 测试用：内存数据库
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn insert_task(&self, rec: &TaskRecord) -> Result<TaskId> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks (url, dir, file_name, user_named, total, resumable, etag, status, error, segments, downloaded, created_at, completed_at, headers)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                rec.url,
                rec.dir.to_string_lossy(),
                rec.file_name,
                rec.user_named as i64,
                rec.total.map(|t| t as i64),
                rec.resumable as i64,
                rec.etag,
                status_str(&rec.status),
                status_error(&rec.status),
                serde_json::to_string(&rec.segments)?,
                rec.downloaded as i64,
                rec.created_at,
                rec.completed_at,
                serde_json::to_string(&rec.headers)?,
            ],
        )?;
        Ok(TaskId(conn.last_insert_rowid() as u64))
    }

    /// 全量更新（状态、文件名、大小、ETag、区段表……）
    pub fn save_task(&self, rec: &TaskRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET url=?2, dir=?3, file_name=?4, user_named=?5, total=?6, resumable=?7, etag=?8,
                              status=?9, error=?10, segments=?11, downloaded=?12, completed_at=?13, headers=?14
             WHERE id=?1",
            params![
                rec.id.0 as i64,
                rec.url,
                rec.dir.to_string_lossy(),
                rec.file_name,
                rec.user_named as i64,
                rec.total.map(|t| t as i64),
                rec.resumable as i64,
                rec.etag,
                status_str(&rec.status),
                status_error(&rec.status),
                serde_json::to_string(&rec.segments)?,
                rec.downloaded as i64,
                rec.completed_at,
                serde_json::to_string(&rec.headers)?,
            ],
        )?;
        Ok(())
    }

    /// 下载中周期性刷新：只写区段表和已下载量
    pub fn save_progress(&self, id: TaskId, segments: &SegmentMap, downloaded: u64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET segments=?2, downloaded=?3 WHERE id=?1",
            params![id.0 as i64, serde_json::to_string(segments)?, downloaded as i64],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: TaskId) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM tasks WHERE id=?1", params![id.0 as i64])?;
        Ok(())
    }

    pub fn load_tasks(&self) -> Result<Vec<TaskRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, url, dir, file_name, user_named, total, resumable, etag, status, error, segments, downloaded, created_at, completed_at, headers
             FROM tasks ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let segments_json: String = row.get(10)?;
            let status_s: String = row.get(8)?;
            let error: Option<String> = row.get(9)?;
            Ok(TaskRecord {
                id: TaskId(row.get::<_, i64>(0)? as u64),
                url: row.get(1)?,
                dir: PathBuf::from(row.get::<_, String>(2)?),
                file_name: row.get(3)?,
                user_named: row.get::<_, i64>(4)? != 0,
                total: row.get::<_, Option<i64>>(5)?.map(|t| t as u64),
                resumable: row.get::<_, i64>(6)? != 0,
                etag: row.get(7)?,
                status: parse_status(&status_s, error),
                segments: serde_json::from_str(&segments_json).unwrap_or_default(),
                downloaded: row.get::<_, i64>(11)? as u64,
                created_at: row.get(12)?,
                completed_at: row.get(13)?,
                headers: serde_json::from_str(&row.get::<_, String>(14)?).unwrap_or_default(),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let v = conn
            .query_row("SELECT value FROM settings WHERE key=?1", params![key], |row| row.get::<_, String>(0))
            .optional()?;
        Ok(v)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

fn status_str(s: &TaskStatus) -> &'static str {
    match s {
        TaskStatus::Queued | TaskStatus::Probing | TaskStatus::Downloading => "downloading",
        TaskStatus::Paused => "paused",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed(_) => "failed",
    }
}

fn status_error(s: &TaskStatus) -> Option<&str> {
    match s {
        TaskStatus::Failed(e) => Some(e.as_str()),
        _ => None,
    }
}

fn parse_status(s: &str, error: Option<String>) -> TaskStatus {
    match s {
        "downloading" => TaskStatus::Downloading,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed(error.unwrap_or_else(|| "未知错误".into())),
        _ => TaskStatus::Paused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(url: &str) -> TaskRecord {
        TaskRecord {
            id: TaskId(0),
            url: url.into(),
            dir: PathBuf::from("/tmp/dl"),
            file_name: "a.bin".into(),
            user_named: false,
            total: Some(1000),
            resumable: true,
            etag: Some("\"abc\"".into()),
            status: TaskStatus::Downloading,
            segments: SegmentMap::plan(1000, 2, 100),
            downloaded: 0,
            created_at: 1_700_000_000,
            completed_at: None,
            headers: vec![("Cookie".into(), "token=1".into())],
        }
    }

    #[test]
    fn task_round_trip() {
        let store = Store::in_memory().unwrap();
        let id = store.insert_task(&rec("https://a/x")).unwrap();
        let id2 = store.insert_task(&rec("https://a/y")).unwrap();
        assert_ne!(id, id2);

        let mut map = SegmentMap::plan(1000, 2, 100);
        map.segments[0].pos = 500;
        store.save_progress(id, &map, 500).unwrap();

        let mut r = rec("https://a/x");
        r.id = id2;
        r.status = TaskStatus::Failed("HTTP 状态码 403".into());
        store.save_task(&r).unwrap();

        let tasks = store.load_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, id);
        assert_eq!(tasks[0].downloaded, 500);
        assert_eq!(tasks[0].segments.segments[0].pos, 500);
        assert_eq!(tasks[0].etag.as_deref(), Some("\"abc\""));
        assert_eq!(tasks[1].status, TaskStatus::Failed("HTTP 状态码 403".into()));
        assert_eq!(tasks[0].headers, vec![("Cookie".to_string(), "token=1".to_string())]);

        store.delete_task(id).unwrap();
        assert_eq!(store.load_tasks().unwrap().len(), 1);
    }

    /// v0.2 的库没有 headers 列：打开时应自动补列，旧任务读出来 headers 为空
    #[test]
    fn migrates_old_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("old.db");
        {
            let old_schema = SCHEMA.replace(",\n    headers      TEXT    NOT NULL DEFAULT '[]'", "");
            assert!(!old_schema.contains("headers"));
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(&old_schema).unwrap();
            conn.execute(
                "INSERT INTO tasks (url, dir, status, created_at) VALUES ('https://a/x', '/tmp', 'paused', 1)",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&db).unwrap();
        let tasks = store.load_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].headers.is_empty());
        // 再开一次不应重复补列
        drop(store);
        Store::open(&db).unwrap();
    }

    #[test]
    fn settings_round_trip() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.get_setting("theme").unwrap(), None);
        store.set_setting("theme", "dark").unwrap();
        store.set_setting("theme", "light").unwrap();
        assert_eq!(store.get_setting("theme").unwrap().as_deref(), Some("light"));
    }
}
