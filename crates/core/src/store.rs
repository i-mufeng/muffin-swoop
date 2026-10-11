//! SQLite 持久化：任务表 + 设置键值表。
//!
//! 引擎在任务新增/状态变化时立即写入，下载中每秒刷新一次区段表。
//! 所有调用都是同步的，内部用 Mutex 串行化；单次写入在毫秒级。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::queue::{MAIN_QUEUE_ID, MAX_QUEUE_CONCURRENCY, QueueId, QueueRecord};
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
    headers      TEXT    NOT NULL DEFAULT '[]',
    description  TEXT    NOT NULL DEFAULT '',
    speed_limit  INTEGER NOT NULL DEFAULT 0,
    source_changed INTEGER NOT NULL DEFAULT 0,
    sync_state TEXT NOT NULL DEFAULT '{}',
    ftp_options TEXT DEFAULT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS queues (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    max_concurrent INTEGER NOT NULL DEFAULT 3,
    state TEXT NOT NULL DEFAULT 'RUNNING',
    sync_enabled INTEGER NOT NULL DEFAULT 0,
    schedule TEXT,
    next_start_at INTEGER,
    next_stop_at INTEGER,
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS queue_members (
    task_id INTEGER PRIMARY KEY,
    queue_id INTEGER NOT NULL,
    position INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS queue_members_order ON queue_members(queue_id, position);
INSERT OR IGNORE INTO queues (id, name) VALUES (1, '主队列');
"#;

/// 建表之后追加的列：旧库启动时按需 `ALTER TABLE` 补上（幂等）
const ADDED_TASK_COLUMNS: [(&str, &str); 6] = [
    ("headers", "TEXT NOT NULL DEFAULT '[]'"),
    ("description", "TEXT NOT NULL DEFAULT ''"),
    ("speed_limit", "INTEGER NOT NULL DEFAULT 0"),
    ("source_changed", "INTEGER NOT NULL DEFAULT 0"),
    ("sync_state", "TEXT NOT NULL DEFAULT '{}'"),
    ("ftp_options", "TEXT DEFAULT NULL"),
];

fn init(conn: &Connection) -> Result<()> {
    let had_queues: bool =
        conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='queues')", [], |row| {
            row.get(0)
        })?;
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
    let has_sync: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('queues') WHERE name='sync_enabled')",
        [],
        |row| row.get(0),
    )?;
    if !has_sync {
        conn.execute_batch("ALTER TABLE queues ADD COLUMN sync_enabled INTEGER NOT NULL DEFAULT 0")?;
    }
    if !had_queues {
        conn.execute(
            "INSERT OR IGNORE INTO queue_members (task_id, queue_id, position) SELECT id, 1, id FROM tasks",
            [],
        )?;
    }
    Ok(())
}

/// 任务在数据库里的完整形态
#[derive(Clone, Debug)]
pub struct TaskRecord {
    /// 0 表示尚未分配（插入时在同一事务内分配不复用的 ID）
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
    pub description: String,
    pub speed_limit: u64,
    /// 地址变更后必须重新核实远端实体，验证成功前不可直接使用旧片段。
    pub source_changed: bool,
    /// 已完成目标的归属凭据与待同步标记；旧库默认为空，不接管现有文件。
    pub sync_state: crate::sync::SyncState,
    /// 非敏感 FTP 任务配置；密码不参与任何持久化。
    pub ftp_options: Option<crate::FtpTaskOptions>,
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
        self.insert_task_in_queue(rec, MAIN_QUEUE_ID)
    }

    pub(crate) fn insert_task_in_queue(&self, rec: &TaskRecord, queue: QueueId) -> Result<TaskId> {
        let mut guard = self.conn.lock().unwrap();
        let conn = guard.transaction()?;
        // SQLite 的普通 INTEGER PRIMARY KEY 会复用刚删除的最大 ID，而旧运行可能仍在收尾。
        // 使用既有 settings 表保存高水位；兼容旧库，同时保证删光任务和重启后也不复用。
        let max_id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM tasks", [], |row| row.get(0))?;
        let high_water: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key='core.task_id_high_water'", [], |row| row.get(0))
            .optional()?;
        let high_water = high_water
            .map(|value| value.parse::<i64>().map_err(|_| Error::Internal("任务 ID 高水位无效".into())))
            .transpose()?
            .unwrap_or(0);
        let next_id = max_id.max(high_water).checked_add(1).ok_or_else(|| Error::Internal("任务 ID 已耗尽".into()))?;
        conn.execute(
            "INSERT INTO tasks (url, dir, file_name, user_named, total, resumable, etag, status, error, segments, downloaded, created_at, completed_at, headers, id, description, speed_limit, source_changed, sync_state, ftp_options)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
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
                next_id,
                rec.description,
                rec.speed_limit.min(i64::MAX as u64) as i64,
                rec.source_changed,
                serde_json::to_string(&rec.sync_state)?,
                rec.ftp_options.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        let id = TaskId(next_id as u64);
        conn.execute(
            "INSERT INTO settings (key,value) VALUES ('core.task_id_high_water',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [next_id.to_string()],
        )?;
        conn.execute(
            "INSERT INTO queue_members (task_id, queue_id, position)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM queue_members WHERE queue_id=?2))",
            params![id.0 as i64, queue.0 as i64],
        )?;
        conn.commit()?;
        Ok(id)
    }

    /// 全量更新（状态、文件名、大小、ETag、区段表……）
    pub fn save_task(&self, rec: &TaskRecord) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET url=?2, dir=?3, file_name=?4, user_named=?5, total=?6, resumable=?7, etag=?8,
                              status=?9, error=?10, segments=?11, downloaded=?12, completed_at=?13, headers=?14, source_changed=?15, sync_state=?16, ftp_options=?17
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
                rec.source_changed,
                serde_json::to_string(&rec.sync_state)?,
                rec.ftp_options.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        Ok(())
    }

    /// 属性独立更新，避免下载进度线程持有的旧快照覆盖刚保存的描述或限速。
    pub(crate) fn save_task_properties(&self, id: TaskId, description: &str, speed_limit: u64) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE tasks SET description=?2, speed_limit=?3 WHERE id=?1",
            params![id.0 as i64, description, speed_limit as i64],
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
        let mut guard = self.conn.lock().unwrap();
        let conn = guard.transaction()?;
        conn.execute("DELETE FROM tasks WHERE id=?1", params![id.0 as i64])?;
        conn.execute("DELETE FROM queue_members WHERE task_id=?1", params![id.0 as i64])?;
        conn.commit()?;
        Ok(())
    }

    pub(crate) fn load_queues(&self) -> Result<Vec<QueueRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name, max_concurrent, state, schedule, next_start_at, next_stop_at, created_at, updated_at, sync_enabled FROM queues ORDER BY id")?;
        let mut queues = stmt
            .query_map([], |row| {
                let schedule: Option<String> = row.get(4)?;
                let schedule = schedule
                    .map(|s| {
                        serde_json::from_str(&s).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
                        })
                    })
                    .transpose()?;
                Ok(QueueRecord {
                    id: QueueId(row.get::<_, i64>(0)? as u64),
                    name: row.get(1)?,
                    max_concurrent: (row.get::<_, i64>(2)?.max(1) as usize).min(MAX_QUEUE_CONCURRENCY),
                    running: row.get::<_, String>(3)? == "RUNNING",
                    task_ids: Vec::new(),
                    schedule,
                    next_start_at: row.get(5)?,
                    next_stop_at: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                    sync_enabled: row.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut members =
            conn.prepare("SELECT task_id FROM queue_members WHERE queue_id=?1 ORDER BY position, task_id")?;
        for q in &mut queues {
            if let Some(schedule) = &q.schedule {
                schedule.validate()?;
            }
            q.task_ids = members
                .query_map([q.id.0 as i64], |row| Ok(TaskId(row.get::<_, i64>(0)? as u64)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }
        Ok(queues)
    }

    /// 队列配置与所有成员顺序一次事务落盘，不影响任务记录与已下载文件。
    pub(crate) fn save_queues<'a>(&self, queues: impl Iterator<Item = &'a QueueRecord>) -> Result<()> {
        let mut guard = self.conn.lock().unwrap();
        let conn = guard.transaction()?;
        conn.execute("DELETE FROM queue_members", [])?;
        conn.execute("DELETE FROM queues", [])?;
        for q in queues {
            conn.execute(
                "INSERT INTO queues (id,name,max_concurrent,state,schedule,next_start_at,next_stop_at,created_at,updated_at,sync_enabled)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![q.id.0 as i64, q.name, q.max_concurrent as i64, if q.running { "RUNNING" } else { "STOPPED" },
                    q.schedule.as_ref().map(serde_json::to_string).transpose()?, q.next_start_at, q.next_stop_at, q.created_at, q.updated_at, q.sync_enabled],
            )?;
            for (position, id) in q.task_ids.iter().enumerate() {
                conn.execute(
                    "INSERT INTO queue_members (task_id,queue_id,position) VALUES (?1,?2,?3)",
                    params![id.0 as i64, q.id.0 as i64, position as i64],
                )?;
            }
        }
        conn.commit()?;
        Ok(())
    }

    pub fn load_tasks(&self) -> Result<Vec<TaskRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, url, dir, file_name, user_named, total, resumable, etag, status, error, segments, downloaded, created_at, completed_at, headers, description, speed_limit, source_changed, sync_state, ftp_options
             FROM tasks ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let segments_json: String = row.get(10)?;
            let status_s: String = row.get(8)?;
            let error: Option<String> = row.get(9)?;
            let ftp_options = row.get::<_, Option<String>>(19)?
                .map(|value| serde_json::from_str(&value).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(19, rusqlite::types::Type::Text, Box::new(e))
                })).transpose()?;
            Ok(TaskRecord {
                ftp_options,
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
                description: row.get(15)?,
                speed_limit: row.get::<_, i64>(16)?.max(0) as u64,
                source_changed: row.get(17)?,
                sync_state: serde_json::from_str(&row.get::<_, String>(18)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(18, rusqlite::types::Type::Text, Box::new(e))
                })?,
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

    /// 设置批量保存：任一键失败时整批回滚，避免 UI 保存了一半配置。
    pub fn set_settings(&self, settings: &[(&str, String)]) -> Result<()> {
        let mut guard = self.conn.lock().unwrap();
        let transaction = guard.transaction()?;
        for (key, value) in settings {
            transaction.execute(
                "INSERT INTO settings (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )?;
        }
        transaction.commit()?;
        Ok(())
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
        TaskStatus::Queued => "queued",
        TaskStatus::Probing | TaskStatus::Downloading => "downloading",
        TaskStatus::Paused => "paused",
        TaskStatus::FileConflict => "file_conflict",
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
        "queued" => TaskStatus::Queued,
        "file_conflict" => TaskStatus::FileConflict,
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
            description: String::new(),
            speed_limit: 0,
            source_changed: false,
            sync_state: Default::default(),
            ftp_options: None,
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
            let old_schema = SCHEMA
                .split("CREATE TABLE IF NOT EXISTS queues")
                .next()
                .unwrap()
                .replace(",\n    headers      TEXT    NOT NULL DEFAULT '[]'", "")
                .replace(",\n    description  TEXT    NOT NULL DEFAULT ''", "")
                .replace(",\n    speed_limit  INTEGER NOT NULL DEFAULT 0", "")
                .replace(",\n    source_changed INTEGER NOT NULL DEFAULT 0", "")
                .replace(",\n    sync_state TEXT NOT NULL DEFAULT '{}'", "")
                .replace(",\n    ftp_options TEXT DEFAULT NULL", "");
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
        assert_eq!(tasks[0].description, "");
        assert_eq!(tasks[0].speed_limit, 0);
        assert!(!tasks[0].source_changed);
        assert!(tasks[0].sync_state.baseline.is_none());
        assert!(!tasks[0].sync_state.pending);
        assert!(tasks[0].ftp_options.is_none());
        let queues = store.load_queues().unwrap();
        assert_eq!(queues.len(), 1);
        assert_eq!(queues[0].id, MAIN_QUEUE_ID);
        assert_eq!(queues[0].task_ids, [tasks[0].id], "旧任务必须全部进入主队列");
        // 再开一次不应重复补列或把用户移出的任务重新加入主队列。
        let mut main = QueueRecord::main();
        main.task_ids.clear();
        store.save_queues([&main].into_iter()).unwrap();
        drop(store);
        let reopened = Store::open(&db).unwrap();
        assert!(reopened.load_queues().unwrap()[0].task_ids.is_empty());
    }

    #[test]
    fn migrates_existing_queues_without_enabling_sync_or_losing_members() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("queues.db");
        let old = SCHEMA
            .replace("    sync_enabled INTEGER NOT NULL DEFAULT 0,\n", "")
            .replace(",\n    sync_state TEXT NOT NULL DEFAULT '{}'", "");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(&old).unwrap();
            conn.execute(
                "INSERT INTO tasks (id,url,dir,status,created_at) VALUES (7,'https://a/file','/tmp','completed',1)",
                [],
            )
            .unwrap();
            conn.execute("INSERT INTO queue_members (task_id,queue_id,position) VALUES (7,1,0)", []).unwrap();
        }
        let store = Store::open(&db).unwrap();
        let queue = store.load_queues().unwrap().remove(0);
        assert!(!queue.sync_enabled);
        assert_eq!(queue.task_ids, [TaskId(7)]);
        assert!(store.load_tasks().unwrap()[0].sync_state.baseline.is_none());
        drop(store);
        assert_eq!(Store::open(&db).unwrap().load_queues().unwrap()[0].task_ids, [TaskId(7)]);
    }

    #[test]
    fn deleting_the_last_task_never_reuses_its_id_even_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("ids.db");
        let store = Store::open(&db).unwrap();
        let first = store.insert_task(&rec("https://a/old")).unwrap();
        store.delete_task(first).unwrap();
        let second = store.insert_task(&rec("https://a/new")).unwrap();
        assert!(second > first);
        store.delete_task(second).unwrap();
        drop(store);
        let store = Store::open(&db).unwrap();
        let third = store.insert_task(&rec("https://a/restarted")).unwrap();
        assert!(third > second);
        let mut stale = rec("https://a/old");
        stale.id = first;
        stale.status = TaskStatus::Failed("旧运行取消".into());
        store.save_task(&stale).unwrap();
        assert_eq!(store.load_tasks().unwrap()[0].url, "https://a/restarted");
        assert_eq!(store.load_queues().unwrap()[0].task_ids, [third]);
    }

    #[test]
    fn batch_settings_roll_back_when_sqlite_rejects_one_key() {
        let store = Store::in_memory().unwrap();
        store.set_setting("first", "old").unwrap();
        store.conn.lock().unwrap().execute_batch(
            "CREATE TRIGGER deny_setting BEFORE INSERT ON settings WHEN NEW.key='denied' BEGIN SELECT RAISE(ABORT, 'settings batch blocked'); END;"
        ).unwrap();
        let error = store.set_settings(&[("first", "updated".into()), ("denied", "new".into())]).unwrap_err();
        assert!(error.to_string().contains("settings batch blocked"), "{error}");
        assert_eq!(store.get_setting("first").unwrap().as_deref(), Some("old"));
        assert_eq!(store.get_setting("denied").unwrap(), None);
        store.set_settings(&[("first", "success".into()), ("second", "saved".into())]).unwrap();
        assert_eq!(store.get_setting("first").unwrap().as_deref(), Some("success"));
        assert_eq!(store.get_setting("second").unwrap().as_deref(), Some("saved"));
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
