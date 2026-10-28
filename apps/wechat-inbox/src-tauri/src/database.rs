use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{InboxRecord, Priority, RecordSource, RecordStatus, ReminderMode, Settings, UnlockReminder, RecordPatch, NullablePatch};
use crate::wechat_client::Session;
use crate::wechat_secrets::protect_context;
use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub struct ReplyJob {
    pub client_id: String,
    pub recipient: String,
    pub context: String,
    pub reply: String,
    pub attempts: u32,
}

pub struct ParseJob {
    pub record_id: String,
    pub account: String,
    pub recipient: String,
    pub context: String,
    pub attempts: u32,
    pub expected_revision: i64,
    pub reference_date: String,
    pub claim_token: String,
}

pub struct WechatStats {
    pub pending: usize,
    pub failed: usize,
    pub last_received: Option<String>,
}

const SAVED_REPLY: &str = "已保存原始文本到拾笺本地收件箱；尚未解析日期，也未设置提醒。";
const PARSING_REPLY: &str = "已收到，正在解析成待办，解析结果稍后回复。";
const UNSUPPORTED_REPLY: &str =
    "这条消息未保存：目前仅支持不超过 50000 字的纯文本；图片、语音、文件和混合消息暂不支持。";

fn message_key(message: &Value) -> Option<String> {
    let sender = message.get("from_user_id")?.as_str()?;
    let id = |name| {
        message.get(name).and_then(|value| {
            value
                .as_u64()
                .filter(|id| *id > 0)
                .map(|id| id.to_string())
                .or_else(|| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
        })
    };
    let identity = if let Some(id) = id("message_id") {
        format!("id:{id}")
    } else if let Some(seq) = id("seq") {
        format!("seq:{seq}")
    } else {
        let timestamp = message.get("create_time_ms")?.as_u64()?;
        let items = message.get("item_list")?;
        format!(
            "fallback:{timestamp}:{}:{items}",
            message
                .get("client_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
        )
    };
    Some(format!(
        "{:x}",
        Sha256::digest(format!("{sender}\0{identity}").as_bytes())
    ))
}

pub struct Database {
    connection: Connection,
}

fn db_error(error: rusqlite::Error) -> String {
    format!("本地数据库操作失败：{error}")
}

fn reference_date(created_at: &str) -> Result<String, String> {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|at| at.with_timezone(&chrono::Local).date_naive().to_string())
        .map_err(|_| "记录日期无效，无法固定解析日期基准".into())
}

fn message_reference_date(message: &Value, fallback: &str) -> Result<String, String> {
    let received = chrono::DateTime::parse_from_rfc3339(fallback).map_err(|_| "接收日期无效")?;
    let sent = message.get("create_time_ms").and_then(Value::as_i64).filter(|millis| *millis > 0)
        .and_then(chrono::DateTime::<Utc>::from_timestamp_millis)
        .filter(|at| *at <= received.with_timezone(&Utc) + chrono::Duration::minutes(5));
    match sent {
        Some(at) => Ok(at.with_timezone(&chrono::Local).date_naive().to_string()),
        None => reference_date(fallback),
    }
}

fn read_record(connection: &Connection, id: &str) -> Result<Option<InboxRecord>, String> {
    let json: Option<String> = connection.query_row("SELECT data FROM records WHERE id=?1", [id], |row| row.get(0)).optional().map_err(db_error)?;
    json.map(|json| serde_json::from_str(&json).map_err(|_| "本地收件记录损坏".into())).transpose()
}

fn write_record(connection: &Connection, record: &InboxRecord) -> Result<(), String> {
    let json = serde_json::to_string(record).map_err(|_| "编码收件记录失败")?;
    if connection.execute("UPDATE records SET data=?2,revision=revision+1 WHERE id=?1", params![record.id,json]).map_err(db_error)? != 1 { return Err("未找到这条收件记录".into()); }
    Ok(())
}

// The caller owns the transaction; keep seen identities to prevent redelivery.
fn delete_record_rows(connection: &Connection, id: &str) -> Result<usize, String> {
    let changed = connection.execute("DELETE FROM records WHERE id=?1", [id]).map_err(db_error)?;
    for table in ["reminder_log", "parse_jobs", "wechat_outbox"] {
        connection.execute(&format!("DELETE FROM {table} WHERE record_id=?1"), [id]).map_err(db_error)?;
    }
    Ok(changed)
}

fn parse_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ParseJob> {
    Ok(ParseJob { record_id:row.get(0)?, account:row.get(1)?, recipient:row.get(2)?, context:row.get(3)?, attempts:row.get(4)?, expected_revision:row.get(5)?, reference_date:row.get(6)?, claim_token:row.get(7)? })
}

fn finalize_parse(connection: &Connection, job: &ParseJob, result: Result<(InboxRecord, String), (String, bool)>, now: i64) -> Result<(), String> {
    let (record, reply) = match result {
        Ok((record, reply)) => (record, reply),
        Err((_reason, false)) if job.attempts < 3 => {
            connection.execute("UPDATE parse_jobs SET claim_token='',next_attempt=?2 WHERE record_id=?1", params![job.record_id, now+60]).map_err(db_error)?;
            return Ok(());
        }
        Err((reason, _)) => {
            let Some(mut record) = read_record(connection, &job.record_id)? else {
                connection.execute("DELETE FROM parse_jobs WHERE record_id=?1", [&job.record_id]).map_err(db_error)?;
                return Ok(());
            };
            record.status = RecordStatus::Failed;
            record.error = Some(format!("模型解析失败：{reason}"));
            let reply = format!("原文已保存，解析失败：{reason}；请在拾笺中手动确认。");
            (record, reply)
        }
    };
    write_record(connection, &record)?;
    connection.execute("DELETE FROM parse_jobs WHERE record_id=?1", [&job.record_id]).map_err(db_error)?;
    connection.execute("INSERT INTO wechat_outbox(client_id,account,recipient,context,reply,state,record_id) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![format!("shijian-parse-{}",job.record_id),job.account,job.recipient,job.context,reply,if job.context.is_empty() {"failed"} else {"pending"},job.record_id]).map_err(db_error)?;
    Ok(())
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::initialize(Connection::open(path).map_err(db_error)?)
    }

    fn initialize(connection: Connection) -> Result<Self, String> {
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(db_error)?;
        if version > 7 {
            return Err("数据库版本高于当前应用，未修改数据；请使用更新版本".into());
        }
        connection.busy_timeout(std::time::Duration::from_secs(5)).map_err(db_error)?;
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;").map_err(db_error)?;
        let tx = connection.unchecked_transaction().map_err(db_error)?;
        // v4 把 reminder_log 从“按 (记录,截止日,日) 追加”改为“每 (记录,截止日) 一行 + 完整时间戳”。
        // 这张表只是提醒去重缓存，丢掉最多导致多提醒一次，因此直接重建而不是逐行搬迁。
        if version < 4 {
            tx
                .execute_batch("DROP TABLE IF EXISTS reminder_log;")
                .map_err(db_error)?;
        }
        tx
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_error)?;
        tx
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS settings (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS records (
                 id TEXT PRIMARY KEY NOT NULL,
                 created_at TEXT NOT NULL,
                 data TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS records_created_at ON records(created_at DESC);
             CREATE TABLE IF NOT EXISTS wechat_cursors (
                 account TEXT PRIMARY KEY NOT NULL,
                 cursor TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS wechat_seen (
                 account TEXT NOT NULL,
                 message_key TEXT NOT NULL,
                 received_at TEXT NOT NULL,
                 PRIMARY KEY (account, message_key)
             );
             CREATE TABLE IF NOT EXISTS wechat_outbox (
                 client_id TEXT PRIMARY KEY NOT NULL,
                 account TEXT NOT NULL,
                 recipient TEXT NOT NULL,
                 context TEXT NOT NULL,
                 reply TEXT NOT NULL,
                 state TEXT NOT NULL,
                 attempts INTEGER NOT NULL DEFAULT 0,
                 next_attempt INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS wechat_outbox_due ON wechat_outbox(account, state, next_attempt);
             CREATE TABLE IF NOT EXISTS reminder_log (
                 record_id TEXT NOT NULL,
                 due_date TEXT NOT NULL,
                 sent_on TEXT NOT NULL,
                 sent_at INTEGER NOT NULL,
                 PRIMARY KEY (record_id, due_date)
             );
             CREATE TABLE IF NOT EXISTS parse_jobs (
                 record_id TEXT PRIMARY KEY NOT NULL,
                 account TEXT NOT NULL,
                 recipient TEXT NOT NULL,
                 context TEXT NOT NULL,
                 attempts INTEGER NOT NULL DEFAULT 0,
                 next_attempt INTEGER NOT NULL DEFAULT 0
             );
             ",
            )
            .map_err(db_error)?;
        let data = serde_json::to_string(&Settings::default())
            .map_err(|e| format!("初始化设置失败：{e}"))?;
        tx
            .execute(
                "INSERT OR IGNORE INTO settings(id, data) VALUES (1, ?1)",
                [data],
            )
            .map_err(db_error)?;
        if version < 6 {
            tx.execute_batch("ALTER TABLE records ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE parse_jobs ADD COLUMN expected_revision INTEGER NOT NULL DEFAULT 0;
                ALTER TABLE parse_jobs ADD COLUMN reference_date TEXT NOT NULL DEFAULT '';
                ALTER TABLE parse_jobs ADD COLUMN claim_token TEXT NOT NULL DEFAULT '';").map_err(db_error)?;
            let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            let rows: Vec<(String, String)> = {
                let mut stmt = tx.prepare("SELECT id,data FROM records").map_err(db_error)?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).map_err(db_error)?;
                rows.collect::<Result<_, _>>().map_err(db_error)?
            };
            for (id, json) in rows {
                let mut record: InboxRecord = serde_json::from_str(&json).map_err(|_| "本地收件记录损坏；迁移未修改数据")?;
                if record.status == RecordStatus::Archived && record.archived_at.is_none() {
                    record.archived_at = Some(now.clone());
                    tx.execute("UPDATE records SET data=?2 WHERE id=?1", params![id, serde_json::to_string(&record).map_err(|_| "编码迁移记录失败")?]).map_err(db_error)?;
                }
                let reference = reference_date(&record.created_at)?;
                tx.execute("UPDATE parse_jobs SET reference_date=?2 WHERE record_id=?1", params![id, reference]).map_err(db_error)?;
            }
            tx.execute_batch("DELETE FROM parse_jobs WHERE record_id NOT IN (SELECT id FROM records); PRAGMA user_version=6;").map_err(db_error)?;
        }
        if version < 7 {
            // Only deterministic parse receipt IDs can be attributed exactly. Never guess
            // the record behind a legacy random first-receipt ID from text or timestamps.
            tx.execute_batch("ALTER TABLE wechat_outbox ADD COLUMN record_id TEXT;
                CREATE INDEX wechat_outbox_record_id ON wechat_outbox(record_id);
                UPDATE wechat_outbox SET record_id=(SELECT id FROM records WHERE client_id='shijian-parse-' || id)
                    WHERE EXISTS (SELECT 1 FROM records WHERE client_id='shijian-parse-' || id);
                DELETE FROM wechat_outbox WHERE substr(client_id,1,14)='shijian-parse-' AND record_id IS NULL;
                DELETE FROM wechat_outbox WHERE record_id IS NULL AND state IN ('pending','failed');
                PRAGMA user_version=7;").map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(Self { connection })
    }

    pub fn settings(&self) -> Result<Settings, String> {
        let json: String = self
            .connection
            .query_row("SELECT data FROM settings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .map_err(db_error)?;
        let mut settings: Settings =
            serde_json::from_str(&json).map_err(|e| format!("本地设置数据损坏：{e}"))?;
        // 0.3 选“每次解锁”的用户迁移为每次解锁提醒；其余沿用新默认值。
        if !json.contains("\"remindOnUnlock\"") && settings.reminder_mode == ReminderMode::Unlock {
            settings.unlock_reminder = UnlockReminder::EveryUnlock;
        }
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        let data = serde_json::to_string(settings).map_err(|e| format!("编码设置失败：{e}"))?;
        let changed = self
            .connection
            .execute("UPDATE settings SET data = ?1 WHERE id = 1", [data])
            .map_err(db_error)?;
        if changed != 1 {
            return Err("本地设置记录缺失".into());
        }
        Ok(())
    }

    pub fn records(&self) -> Result<Vec<InboxRecord>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT data FROM records ORDER BY created_at DESC, id DESC")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_error)?;
        rows.map(|row| {
            let json = row.map_err(db_error)?;
            serde_json::from_str(&json).map_err(|e| format!("本地收件记录损坏：{e}"))
        })
        .collect()
    }

    pub fn record(&self, id: &str) -> Result<InboxRecord, String> {
        let json: Option<String> = self
            .connection
            .query_row("SELECT data FROM records WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_error)?;
        serde_json::from_str(&json.ok_or("未找到这条收件记录")?)
            .map_err(|e| format!("本地收件记录损坏：{e}"))
    }

    pub fn insert_record(&self, record: &InboxRecord) -> Result<(), String> {
        let data = serde_json::to_string(record).map_err(|e| format!("编码收件记录失败：{e}"))?;
        self.connection
            .execute(
                "INSERT INTO records(id, created_at, data) VALUES (?1, ?2, ?3)",
                params![record.id, record.created_at, data],
            )
            .map_err(db_error)?;
        Ok(())
    }

    /// Human changes invalidate older background claims; never resurrect deleted rows.
    pub fn save_record(&self, record: &InboxRecord) -> Result<(), String> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let current = read_record(&tx, &record.id)?.ok_or("未找到这条收件记录")?;
        if current != *record {
            write_record(&tx, record)?;
            tx.execute("DELETE FROM parse_jobs WHERE record_id=?1", [&record.id]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }

    pub fn patch_record(&self, id: &str, patch: RecordPatch) -> Result<InboxRecord, String> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let original = read_record(&tx, id)?.ok_or("未找到这条收件记录")?;
        if matches!(original.status, RecordStatus::Archived | RecordStatus::Completed) {
            return Err("请先重新打开或恢复待办再编辑".into());
        }
        let mut record = original.clone();
        if let Some(title) = patch.title {
            record.title = crate::validation::text(&title, "标题", crate::validation::MAX_TITLE)?;
        }
        if let NullablePatch::Value(value) = patch.due_date {
            record.due_date = crate::validation::due_date(value)?;
            if record.due_date.is_none() { record.due_time = None; }
        }
        if let NullablePatch::Value(value) = patch.due_time {
            record.due_time = crate::validation::due_time(value, record.due_date.as_deref())?;
        }
        record.due_time = crate::validation::due_time(record.due_time, record.due_date.as_deref())?;
        if let Some(priority) = patch.priority { record.priority = priority; }
        if record != original {
            if matches!(record.status, RecordStatus::Failed | RecordStatus::NeedsReview) {
                record.status = RecordStatus::Saved;
                record.error = None;
            }
            write_record(&tx, &record)?;
            tx.execute("DELETE FROM parse_jobs WHERE record_id=?1", [id]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(record)
    }

    pub fn wechat_cursor(&self, account: &str) -> Result<String, String> {
        Ok(self
            .connection
            .query_row(
                "SELECT cursor FROM wechat_cursors WHERE account = ?1",
                [account],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?
            .unwrap_or_default())
    }

    pub fn ingest_wechat(&mut self, session: &Session, response: &Value) -> Result<usize, String> {
        crate::wechat_client::check_response(response).map_err(|error| error.message())?;
        let parsing_enabled = self.settings()?.parsing_enabled;
        let empty = Vec::new();
        let messages = match response.get("msgs") {
            Some(Value::Array(messages)) => messages,
            None if response.get("ret").and_then(Value::as_i64) == Some(0) => &empty,
            _ => return Err("微信消息列表格式无效；未推进游标".into()),
        };
        if messages.len() > 1000 {
            return Err("单批微信消息过多；未推进游标".into());
        }
        let cursor = match response.get("get_updates_buf") {
            Some(Value::String(cursor)) => cursor.as_str(),
            None => "",
            _ => return Err("微信消息游标格式无效；未推进游标".into()),
        };
        if cursor.len() > 256_000 {
            return Err("微信消息游标过长".into());
        }
        let tx = self.connection.transaction().map_err(db_error)?;
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let mut inserted = 0;
        for message in messages {
            if message.get("message_type").and_then(Value::as_u64) != Some(1)
                || message.get("from_user_id").and_then(Value::as_str)
                    != Some(session.user_id.as_str())
                || message
                    .get("group_id")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
                || message
                    .get("to_user_id")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty() && s != session.bot_id)
                || message.get("message_state").and_then(Value::as_u64) == Some(1)
            {
                continue;
            }
            let Some(key) = message_key(message) else {
                continue;
            };
            let Some(items) = message
                .get("item_list")
                .and_then(Value::as_array)
                .filter(|items| !items.is_empty())
            else {
                continue;
            };
            if items
                .iter()
                .any(|item| item.get("is_completed").and_then(Value::as_bool) == Some(false))
            {
                continue;
            }
            let pure_text = items.iter().all(|item| {
                item.get("type").and_then(Value::as_u64) == Some(1)
                    && item
                        .get("text_item")
                        .and_then(|item| item.get("text"))
                        .and_then(Value::as_str)
                        .is_some()
            });
            let text = items
                .iter()
                .filter_map(|item| {
                    item.get("text_item")
                        .and_then(|item| item.get("text"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join("\n");
            if pure_text && text.trim().is_empty() {
                continue;
            }
            let fresh = tx.execute("INSERT OR IGNORE INTO wechat_seen(account,message_key,received_at) VALUES (?1,?2,?3)",params![session.bot_id,key,now]).map_err(db_error)?;
            if fresh == 0 {
                continue;
            }
            let supported = pure_text
                && !text.trim().is_empty()
                && text.chars().count() <= crate::validation::MAX_ORIGINAL_TEXT
                && !text.contains('\0');
            let mut created_record_id: Option<String> = None;
            let mut parsed_record_id: Option<String> = None;
            let reply = if supported {
                let record = InboxRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    title: text
                        .trim()
                        .chars()
                        .take(crate::validation::MAX_TITLE)
                        .collect(),
                    original_text: text,
                    status: RecordStatus::Saved,
                    source: RecordSource::Wechat,
                    created_at: now.clone(),
                    due_date: None,
                    priority: Priority::Normal,
                    error: None,
                    is_demo: false,
                    status_before_archive: None,
            archived_at: None,
                    due_time: None,
                };
                let data = serde_json::to_string(&record).map_err(|_| "无法编码微信收件记录")?;
                tx.execute(
                    "INSERT INTO records(id,created_at,data) VALUES (?1,?2,?3)",
                    params![record.id, record.created_at, data],
                )
                .map_err(db_error)?;
                inserted += 1;
                created_record_id = Some(record.id.clone());
                if parsing_enabled {
                    parsed_record_id = Some(record.id.clone());
                    PARSING_REPLY
                } else {
                    SAVED_REPLY
                }
            } else {
                UNSUPPORTED_REPLY
            };
            let context = message
                .get("context_token")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if context.len() > 65_536 {
                return Err("微信回复上下文过长；本批事务已回滚".into());
            }
            let protected = protect_context(context)?;
            tx.execute("INSERT INTO wechat_outbox(client_id,account,recipient,context,reply,state,record_id) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![format!("shijian-{}",uuid::Uuid::new_v4()),session.bot_id,session.user_id,protected,reply,if context.is_empty() {"failed"} else {"pending"},created_record_id]).map_err(db_error)?;
            if let Some(record_id) = parsed_record_id {
                // 解析队列持有自己的上下文副本；首条回执发出后 outbox 上下文会被清空。
                tx.execute("INSERT INTO parse_jobs(record_id,account,recipient,context,reference_date) VALUES (?1,?2,?3,?4,?5)",params![record_id,session.bot_id,session.user_id,protected,message_reference_date(message, &now)?]).map_err(db_error)?;
            }
        }
        if !cursor.is_empty() {
            tx.execute("INSERT INTO wechat_cursors(account,cursor) VALUES (?1,?2) ON CONFLICT(account) DO UPDATE SET cursor=excluded.cursor",params![session.bot_id,cursor]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(inserted)
    }

    pub fn wechat_stats(&self, account: &str) -> Result<WechatStats, String> {
        let count = |state| {
            self.connection
                .query_row(
                    "SELECT COUNT(*) FROM wechat_outbox WHERE account=?1 AND state=?2",
                    params![account, state],
                    |row| row.get::<_, usize>(0),
                )
                .map_err(db_error)
        };
        Ok(WechatStats {
            pending: count("pending")?,
            failed: count("failed")?,
            last_received: self
                .connection
                .query_row(
                    "SELECT MAX(received_at) FROM wechat_seen WHERE account=?1",
                    [account],
                    |row| row.get(0),
                )
                .map_err(db_error)?,
        })
    }

    pub fn next_reply(&self, account: &str) -> Result<Option<ReplyJob>, String> {
        self.connection.query_row("SELECT client_id,recipient,context,reply,attempts FROM wechat_outbox WHERE account=?1 AND state='pending' AND next_attempt<=?2 ORDER BY rowid LIMIT 1",params![account,Utc::now().timestamp()],|row| Ok(ReplyJob { client_id:row.get(0)?,recipient:row.get(1)?,context:row.get(2)?,reply:row.get(3)?,attempts:row.get(4)? })).optional().map_err(db_error)
    }

    pub fn start_reply_attempt(&self, id: &str) -> Result<bool, String> {
        let now = Utc::now().timestamp();
        let changed = self.connection.execute(
            "UPDATE wechat_outbox SET attempts=attempts+1,next_attempt=?2
             WHERE client_id=?1 AND state='pending' AND attempts<3 AND next_attempt<=?3
             AND (record_id IS NULL OR EXISTS (SELECT 1 FROM records WHERE id=wechat_outbox.record_id))",
            params![id, now + 60, now],
        ).map_err(db_error)?;
        Ok(changed == 1)
    }

    pub fn finish_reply(&self, id: &str, success: bool, permanent: bool) -> Result<(), String> {
        self.connection.execute("UPDATE wechat_outbox SET state=CASE WHEN ?2 THEN 'sent' WHEN ?3 OR attempts>=3 THEN 'failed' ELSE 'pending' END, next_attempt=?4, context=CASE WHEN ?2 THEN '' ELSE context END WHERE client_id=?1",params![id,success,permanent,Utc::now().timestamp()+10]).map_err(db_error)?;
        Ok(())
    }

    pub fn retry_replies(&self, account: &str) -> Result<(), String> {
        let changed = self.connection.execute("UPDATE wechat_outbox SET state='pending',attempts=0,next_attempt=0 WHERE account=?1 AND state='failed' AND context<>''",[account]).map_err(db_error)?;
        if changed == 0 {
            return Err("没有可重试的回执；缺少回复上下文的消息无法补发，请在微信重新发送".into());
        }
        Ok(())
    }

    pub fn clear_wechat_contexts(&self) -> Result<(), String> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        tx.execute_batch("DELETE FROM parse_jobs; DELETE FROM wechat_outbox;").map_err(db_error)?;
        tx.commit().map_err(db_error)?;
        Ok(())
    }

    pub fn set_archived(&self, id: &str, archived: bool) -> Result<InboxRecord, String> {
        let mut record = self.record(id)?;
        if archived {
            // 重复归档保持原样，不覆盖最初保存的归档前状态。
            if record.status == RecordStatus::Archived {
                return Ok(record);
            }
            record.status_before_archive = Some(record.status);
            record.status = RecordStatus::Archived;
            record.archived_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        } else {
            if record.status != RecordStatus::Archived { return Ok(record); }
            record.archived_at = None;
            let restored = record
                .status_before_archive
                .take()
                .unwrap_or(RecordStatus::Saved);
            record.status = restored;
            // 待确认/失败的记录依靠错误说明原因，恢复时保留；其余清空。
            if matches!(restored, RecordStatus::Saved | RecordStatus::Completed) {
                record.error = None;
            }
        }
        self.save_record(&record)?;
        Ok(record)
    }

    pub fn set_completed(&self, id: &str, completed: bool) -> Result<InboxRecord, String> {
        let mut record = self.record(id)?;
        if record.status == RecordStatus::Archived { return Err("请先恢复归档记录".into()); }
        record.status = if completed {
            RecordStatus::Completed
        } else {
            RecordStatus::Saved
        };
        record.error = None;
        self.save_record(&record)?;
        Ok(record)
    }

    pub fn delete_record(&self, id: &str) -> Result<(), String> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        if delete_record_rows(&tx, id)? != 1 {
            return Err("未找到这条收件记录".into());
        }
        tx.commit().map_err(db_error)
    }

    /// 自动清除只针对已归档且超过保留天数的记录；进行中的记录永不自动删除。
    pub fn auto_clear_archived(&self, cutoff: &str) -> Result<usize, String> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let mut statement = tx
            .prepare("SELECT id, data FROM records")
            .map_err(db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_error)?;
        let mut stale = Vec::new();
        for row in rows {
            let (id, data) = row.map_err(db_error)?;
            let Ok(record) = serde_json::from_str::<InboxRecord>(&data) else {
                continue;
            };
            let older = record.archived_at.as_deref().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .zip(chrono::DateTime::parse_from_rfc3339(cutoff).ok()).is_some_and(|(at, cutoff)| at < cutoff);
            if record.status == RecordStatus::Archived && older {
                stale.push(id);
            }
        }
        drop(statement);
        let mut cleared = 0;
        for id in stale {
            cleared += delete_record_rows(&tx, &id)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(cleared)
    }

    /// 记录某条记录在某个截止日期下最近一次提醒；同一 (记录, 截止日) 只保留最新一条。
    pub fn mark_reminded(
        &self,
        record_id: &str,
        due_date: &str,
        sent_on: chrono::NaiveDate,
        sent_at: i64,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO reminder_log(record_id, due_date, sent_on, sent_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(record_id, due_date) DO UPDATE SET sent_on = excluded.sent_on, sent_at = excluded.sent_at",
                params![record_id, due_date, sent_on.format("%Y-%m-%d").to_string(), sent_at],
            )
            .map_err(db_error)?;
        Ok(())
    }

    /// 最近一次提醒的 (本地日期, unix 秒)；没有记录时返回 None。
    pub fn last_reminder(
        &self,
        record_id: &str,
        due_date: &str,
    ) -> Result<Option<(String, i64)>, String> {
        self.connection
            .query_row(
                "SELECT sent_on, sent_at FROM reminder_log WHERE record_id = ?1 AND due_date = ?2",
                params![record_id, due_date],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_error)
    }

    pub fn prune_reminder_log(&self, before: &str) -> Result<usize, String> {
        self.connection
            .execute("DELETE FROM reminder_log WHERE sent_on < ?1", [before])
            .map_err(db_error)
    }

    /// Atomically lease a valid job for the current account. Crashed leases count as attempts.
    #[cfg(test)]
    pub fn claim_parse_job(&self, account: &str, now: i64) -> Result<Option<ParseJob>, String> {
        self.claim_parse_job_with_changes(account, now).map(|(job, _)| job)
    }

    pub fn claim_parse_job_with_changes(&self, account: &str, now: i64) -> Result<(Option<ParseJob>, bool), String> {
        let mut changed = false;
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        tx.execute("DELETE FROM parse_jobs WHERE NOT EXISTS (SELECT 1 FROM records r WHERE r.id=parse_jobs.record_id AND r.revision=parse_jobs.expected_revision)", []).map_err(db_error)?;
        loop {
            let job = tx.query_row("SELECT record_id,account,recipient,context,attempts,expected_revision,reference_date,claim_token FROM parse_jobs WHERE account=?1 AND next_attempt<=?2 ORDER BY rowid LIMIT 1", params![account,now], parse_job_row).optional().map_err(db_error)?;
            let Some(mut job) = job else { tx.commit().map_err(db_error)?; return Ok((None, changed)); };
            let record = read_record(&tx, &job.record_id)?.ok_or("解析记录缺失")?;
            if matches!(record.status, RecordStatus::Completed | RecordStatus::Archived) {
                tx.execute("DELETE FROM parse_jobs WHERE record_id=?1", [&job.record_id]).map_err(db_error)?;
                continue;
            }
            if job.attempts >= 3 {
                finalize_parse(&tx, &job, Err(("解析尝试已达三次上限（包括中断或崩溃恢复）".into(), true)), now)?;
                changed = true;
                continue;
            }
            job.attempts += 1;
            job.claim_token = uuid::Uuid::new_v4().to_string();
            tx.execute("UPDATE parse_jobs SET attempts=?2,claim_token=?3,next_attempt=?4 WHERE record_id=?1", params![job.record_id,job.attempts,job.claim_token,now+120]).map_err(db_error)?;
            tx.commit().map_err(db_error)?;
            return Ok((Some(job), changed));
        }
    }

    pub fn commit_parse(&self, job: &ParseJob, result: Result<(InboxRecord, String), (String, bool)>, now: i64) -> Result<bool, String> {
        let tx = self.connection.unchecked_transaction().map_err(db_error)?;
        let current = tx.query_row("SELECT 1 FROM parse_jobs j JOIN records r ON r.id=j.record_id WHERE j.record_id=?1 AND j.claim_token=?2 AND j.expected_revision=?3 AND r.revision=?3", params![job.record_id,job.claim_token,job.expected_revision], |_| Ok(())).optional().map_err(db_error)?;
        if current.is_none() { return Ok(false); }
        let record = read_record(&tx, &job.record_id)?.ok_or("解析记录缺失")?;
        if matches!(record.status, RecordStatus::Completed | RecordStatus::Archived) {
            tx.execute("DELETE FROM parse_jobs WHERE record_id=?1", [&job.record_id]).map_err(db_error)?;
            tx.commit().map_err(db_error)?;
            return Ok(false);
        }
        finalize_parse(&tx, job, result, now)?;
        tx.commit().map_err(db_error)?;
        Ok(true)
    }

    /// User cancellation is not a failed attempt; release and refund active leases.
    pub fn release_parse_claims(&self) -> Result<(), String> {
        self.connection.execute("UPDATE parse_jobs SET attempts=MAX(0,attempts-1),claim_token='',next_attempt=0 WHERE claim_token<>''", []).map_err(db_error)?;
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Priority, RecordSource};

    fn outbox_row(db: &Database, id: &str, record_id: Option<&str>, state: &str) {
        db.connection.execute("INSERT INTO wechat_outbox(client_id,account,recipient,context,reply,state,record_id) VALUES (?1,'test-bot','owner','fake-context','receipt',?2,?3)", params![id,state,record_id]).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn first_receipts_link_records_with_parsing_on_and_off_and_results_link_too() {
        for enabled in [false, true] {
            let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
            let mut settings = db.settings().unwrap(); settings.parsing_enabled = enabled;
            db.save_settings(&settings).unwrap();
            db.ingest_wechat(&session(), &batch(vec![incoming(1)], "next")).unwrap();
            let record = db.records().unwrap().remove(0);
            let first = db.next_reply("test-bot").unwrap().unwrap();
            let linked: String = db.connection.query_row("SELECT record_id FROM wechat_outbox WHERE client_id=?1", [&first.client_id], |r| r.get(0)).unwrap();
            assert_eq!(linked, record.id);
            assert_ne!(first.client_id, format!("shijian-{}", record.id));
            let job = db.claim_parse_job("test-bot", Utc::now().timestamp()).unwrap();
            assert_eq!(job.is_some(), enabled);
            if let Some(job) = job {
                assert!(db.commit_parse(&job, Ok((record.clone(), "结果摘要".into())), 1).unwrap());
                let linked: String = db.connection.query_row("SELECT record_id FROM wechat_outbox WHERE client_id=?1", [format!("shijian-parse-{}",record.id)], |r| r.get(0)).unwrap();
                assert_eq!(linked, record.id);
            }
            let mut unsupported = incoming(2);
            unsupported["item_list"] = serde_json::json!([{"type":2,"image_item":{}}]);
            db.ingest_wechat(&session(), &batch(vec![unsupported], "next2")).unwrap();
            let nulls: usize = db.connection.query_row("SELECT COUNT(*) FROM wechat_outbox WHERE record_id IS NULL", [], |r| r.get(0)).unwrap();
            assert_eq!(nulls, 1);
        }
    }

    #[test]
    #[cfg(windows)]
    fn deletion_clears_all_linked_state_preserves_seen_and_blocks_redelivery() {
        for automatic in [false, true] {
            let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
            let mut settings = db.settings().unwrap(); settings.parsing_enabled = true;
            db.save_settings(&settings).unwrap();
            db.ingest_wechat(&session(), &batch(vec![incoming(1)], "next")).unwrap();
            let record = db.records().unwrap().remove(0);
            let parse = db.claim_parse_job("test-bot", Utc::now().timestamp()).unwrap().unwrap();
            let reply = db.next_reply("test-bot").unwrap().unwrap();
            for state in ["pending", "failed", "sent"] { outbox_row(&db, state, Some(&record.id), state); }
            let other = queued_record(&db);
            db.mark_reminded(&other.id, "2026-10-24", date(2026,10,24), 1).unwrap();
            outbox_row(&db, "other", Some(&other.id), "pending");
            outbox_row(&db, "unsupported", None, "pending");
            db.mark_reminded(&record.id, "2026-10-24", date(2026,10,24), 1).unwrap();
            if automatic {
                let mut archived = record.clone(); archived.status = RecordStatus::Archived;
                archived.archived_at = Some("2000-01-01T00:00:00Z".into());
                // Preserve the active parse job to exercise auto-clear's own cleanup.
                db.connection.execute("UPDATE records SET data=?2 WHERE id=?1", params![record.id,serde_json::to_string(&archived).unwrap()]).unwrap();
                assert_eq!(db.auto_clear_archived("2026-01-01T00:00:00Z").unwrap(), 1);
            } else { db.delete_record(&record.id).unwrap(); }
            assert!(!db.start_reply_attempt(&reply.client_id).unwrap());
            db.finish_reply(&reply.client_id, true, false).unwrap();
            assert!(!db.commit_parse(&parse, Ok((record.clone(), "迟到结果".into())), 2).unwrap());
            assert!(!db.commit_parse(&parse, Err(("迟到错误".into(),true)), 2).unwrap());
            assert!(db.record(&record.id).is_err());
            for table in ["reminder_log", "parse_jobs", "wechat_outbox"] {
                let count: usize = db.connection.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE record_id=?1"), [&record.id], |r| r.get(0)).unwrap();
                assert_eq!(count, 0, "{table}");
            }
            assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 2);
            assert_eq!(db.record(&other.id).unwrap(), other);
            assert!(db.last_reminder(&other.id, "2026-10-24").unwrap().is_some());
            let other_jobs: usize = db.connection.query_row("SELECT COUNT(*) FROM parse_jobs WHERE record_id=?1", [&other.id], |r| r.get(0)).unwrap();
            assert_eq!(other_jobs, 1);
            assert_eq!(db.ingest_wechat(&session(), &batch(vec![incoming(1)], "replay")).unwrap(), 0);
            assert_eq!(db.records().unwrap().len(), 1);
            let seen: usize = db.connection.query_row("SELECT COUNT(*) FROM wechat_seen", [], |r| r.get(0)).unwrap();
            assert_eq!(seen, 1);
            assert!(db.delete_record(&record.id).is_err());
        }
    }

    #[test]
    fn deletion_rolls_back_every_related_table_when_cleanup_fails() {
        for automatic in [false, true] {
            let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
            let mut record = sample_record();
            if automatic {
                record.status = RecordStatus::Archived;
                record.archived_at = Some("2000-01-01T00:00:00Z".into());
            }
            db.insert_record(&record).unwrap();
            db.connection.execute("INSERT INTO parse_jobs(record_id,account,recipient,context) VALUES (?1,'test-bot','owner','fake')", [&record.id]).unwrap();
            db.mark_reminded(&record.id, "2026-10-24", date(2026,10,24), 1).unwrap();
            outbox_row(&db, "linked", Some(&record.id), "pending");
            db.connection.execute_batch("CREATE TRIGGER reject_cleanup BEFORE DELETE ON wechat_outbox BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
            let result = if automatic { db.auto_clear_archived("2026-01-01T00:00:00Z").map(|_| ()) } else { db.delete_record(&record.id) };
            assert!(result.is_err());
            assert_eq!(db.record(&record.id).unwrap(), record);
            for table in ["reminder_log", "parse_jobs", "wechat_outbox"] {
                let count: usize = db.connection.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE record_id=?1"), [&record.id], |r| r.get(0)).unwrap();
                assert_eq!(count, 1, "{table}");
            }
        }
    }

    #[test]
    fn reply_claim_rejects_invalid_states_or_missing_records_and_leases_once() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        for state in ["failed", "sent"] {
            outbox_row(&db, state, None, state);
            assert!(!db.start_reply_attempt(state).unwrap());
        }
        outbox_row(&db, "orphan", Some("missing"), "pending");
        assert!(!db.start_reply_attempt("orphan").unwrap());
        assert!(!db.start_reply_attempt("missing").unwrap());
        outbox_row(&db, "unsupported", None, "pending");
        assert!(db.start_reply_attempt("unsupported").unwrap());
        assert!(!db.start_reply_attempt("unsupported").unwrap());
        outbox_row(&db, "exhausted", None, "pending");
        db.connection.execute("UPDATE wechat_outbox SET attempts=3 WHERE client_id='exhausted'", []).unwrap();
        assert!(!db.start_reply_attempt("exhausted").unwrap());
    }

    // Recreate an actual v6 outbox without record_id while retaining the v6 job schema.
    fn as_v6(db: Database) -> Connection {
        db.connection.execute_batch("DROP INDEX wechat_outbox_record_id; ALTER TABLE wechat_outbox DROP COLUMN record_id; PRAGMA user_version=6;").unwrap();
        db.connection
    }

    #[test]
    fn v7_migration_links_only_exact_parse_ids_cancels_unknown_unsent_and_is_idempotent() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = queued_record(&db);
        let failed_record = sample_record(); db.insert_record(&failed_record).unwrap();
        let sent_record = sample_record(); db.insert_record(&sent_record).unwrap();
        db.connection.execute("INSERT INTO wechat_seen VALUES ('test-bot','seen','2026-10-24')", []).unwrap();
        let connection = as_v6(db);
        for (id, state) in [
            (format!("shijian-parse-{}",record.id), "pending"),
            (format!("shijian-parse-{}",failed_record.id), "failed"),
            (format!("shijian-parse-{}",sent_record.id), "sent"),
            ("shijian-parse-orphan-pending".into(), "pending"),
            ("shijian-parse-orphan-failed".into(), "failed"),
            ("shijian-parse-orphan-sent".into(), "sent"),
            ("random-pending".into(), "pending"), ("random-failed".into(), "failed"),
            ("random-sent".into(), "sent"),
            (format!("shijian-parse-{}-extra",record.id), "sent"),
        ] {
            connection.execute("INSERT INTO wechat_outbox(client_id,account,recipient,context,reply,state) VALUES (?1,'test-bot','owner','fake','generic',?2)", params![id,state]).unwrap();
        }
        let mut db = Database::initialize(connection).unwrap();
        // Check both the initial migration and a subsequent open, without assuming UUID order.
        for reopen in [false, true] {
            if reopen { db = Database::initialize(db.connection).unwrap(); }
            let count: usize = db.connection.query_row("SELECT COUNT(*) FROM wechat_outbox", [], |r| r.get(0)).unwrap();
            assert_eq!(count, 4);
            for (expected, state) in [(&record,"pending"), (&failed_record,"failed"), (&sent_record,"sent")] {
                assert_eq!(db.record(&expected.id).unwrap(), *expected);
                let row: (String,String) = db.connection.query_row("SELECT record_id,state FROM wechat_outbox WHERE client_id=?1", [format!("shijian-parse-{}",expected.id)], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
                assert_eq!(row, (expected.id.clone(),state.into()));
            }
            let unlinked: Option<String> = db.connection.query_row("SELECT record_id FROM wechat_outbox WHERE client_id='random-sent'", [], |r| r.get(0)).unwrap();
            assert!(unlinked.is_none());
            let seen: usize = db.connection.query_row("SELECT COUNT(*) FROM wechat_seen", [], |r| r.get(0)).unwrap();
            assert_eq!(seen, 1);
            let version: u32 = db.connection.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(version, 7);
        }
        let db = Database::initialize(db.connection).unwrap();
        let count: usize = db.connection.query_row("SELECT COUNT(*) FROM wechat_outbox", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 4);
        assert!(db.claim_parse_job("test-bot", 1).unwrap().is_some());
        assert!(db.retry_replies("test-bot").is_ok());
        for id in ["random-pending", "random-failed"] { assert!(!db.start_reply_attempt(id).unwrap()); }
    }

    #[test]
    fn v7_migration_rolls_back_linking_deletion_column_index_and_version() {
        let path = std::env::temp_dir().join(format!("shijian-v7-{}.sqlite3",uuid::Uuid::new_v4()));
        let db = Database::open(&path).unwrap();
        let record = sample_record(); db.insert_record(&record).unwrap();
        outbox_row(&db, &format!("shijian-parse-{}",record.id), None, "pending");
        outbox_row(&db, "legacy", None, "failed");
        let connection = as_v6(db);
        connection.execute_batch("CREATE TRIGGER reject_migration BEFORE DELETE ON wechat_outbox BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(Database::initialize(connection).is_err());
        let connection = Connection::open(&path).unwrap();
        let version: u32 = connection.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 6);
        assert!(connection.prepare("SELECT record_id FROM wechat_outbox").is_err());
        let indexes: usize = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='wechat_outbox_record_id'", [], |r| r.get(0)).unwrap();
        assert_eq!(indexes, 0);
        let count: usize = connection.query_row("SELECT COUNT(*) FROM wechat_outbox", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 2);
        connection.execute_batch("DROP TRIGGER reject_migration;").unwrap();
        let db = Database::initialize(connection).unwrap();
        assert_eq!(db.records().unwrap(), vec![record]);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    fn queued_record(db: &Database) -> InboxRecord {
        let record = sample_record();
        db.insert_record(&record).unwrap();
        db.connection.execute("INSERT INTO parse_jobs(record_id,account,recipient,context,reference_date) VALUES (?1,'test-bot','owner','protected-context','2026-10-24')", [&record.id]).unwrap();
        record
    }

    #[test]
    fn patches_preserve_other_fields_and_cancel_old_results() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = queued_record(&db);
        let job = db.claim_parse_job("test-bot", 1).unwrap().unwrap();
        let title: RecordPatch = serde_json::from_str(r#"{"title":"人工标题"}"#).unwrap();
        db.patch_record(&record.id, title).unwrap();
        let priority: RecordPatch = serde_json::from_str(r#"{"priority":"high"}"#).unwrap();
        let latest = db.patch_record(&record.id, priority).unwrap();
        assert_eq!(latest.title, "人工标题");
        assert_eq!(latest.priority, Priority::High);
        assert!(!db.commit_parse(&job, Ok((record.clone(), "旧回执".into())), 2).unwrap());
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 0);
        assert_eq!(db.record(&record.id).unwrap(), latest);
    }

    #[test]
    fn date_patch_distinguishes_missing_null_and_invalid_time() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut record = sample_record();
        record.due_date = Some("2028-02-29".into()); record.due_time = Some("15:00".into());
        db.insert_record(&record).unwrap();
        let latest = db.patch_record(&record.id, serde_json::from_str(r#"{"priority":"high"}"#).unwrap()).unwrap();
        assert_eq!(latest.due_time.as_deref(), Some("15:00"));
        assert!(db.patch_record(&record.id, serde_json::from_str(r#"{"dueDate":null,"dueTime":"16:00"}"#).unwrap()).is_err());
        let cleared = db.patch_record(&record.id, serde_json::from_str(r#"{"dueDate":null}"#).unwrap()).unwrap();
        assert!(cleared.due_date.is_none() && cleared.due_time.is_none());
        assert!(serde_json::from_str::<RecordPatch>(r#"{"status":"saved"}"#).is_err());
    }

    #[test]
    fn lifecycle_changes_reject_delayed_success_and_failure() {
        for action in ["complete", "archive", "restore", "delete", "disconnect"] {
            let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
            let record = queued_record(&db);
            let job = db.claim_parse_job("test-bot", 1).unwrap().unwrap();
            match action {
                "complete" => { db.set_completed(&record.id, true).unwrap(); }
                "archive" => { db.set_archived(&record.id, true).unwrap(); }
                "restore" => { db.set_archived(&record.id, true).unwrap(); db.set_archived(&record.id, false).unwrap(); }
                "delete" => { db.delete_record(&record.id).unwrap(); }
                _ => { db.clear_wechat_contexts().unwrap(); }
            }
            assert!(!db.commit_parse(&job, Ok((record.clone(), "旧结果".into())), 2).unwrap());
            assert!(!db.commit_parse(&job, Err(("旧失败".into(), true)), 2).unwrap());
            assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 0);
            if action != "delete" { assert_ne!(db.record(&record.id).unwrap().status, RecordStatus::Failed); }
        }
    }

    #[test]
    fn meaningless_model_json_retries_three_times_then_links_failure_receipt() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = queued_record(&db);
        for attempt in 0..3 {
            let now = 1 + attempt * 61;
            let job = db.claim_parse_job("test-bot", now).unwrap().unwrap();
            let error = crate::parser::parse_intent_json(r#"{"error":"unavailable"}"#).unwrap_err();
            assert!(!error.is_permanent());
            assert!(db.commit_parse(&job, Err((error.message().into(), error.is_permanent())), now).unwrap());
            let outbox: usize = db.connection.query_row("SELECT COUNT(*) FROM wechat_outbox", [], |r| r.get(0)).unwrap();
            assert_eq!(outbox, if attempt == 2 { 1 } else { 0 });
        }
        assert!(db.claim_parse_job("test-bot", 1000).unwrap().is_none());
        assert_eq!(db.record(&record.id).unwrap().status, RecordStatus::Failed);
        let linked: String = db.connection.query_row("SELECT record_id FROM wechat_outbox", [], |r| r.get(0)).unwrap();
        assert_eq!(linked, record.id);
    }

    #[test]
    fn retry_cap_and_atomic_failure_reply() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = queued_record(&db);
        for attempt in 0..3 {
            let now = 1 + attempt * 61;
            let job = db.claim_parse_job("test-bot", now).unwrap().unwrap();
            assert_eq!(job.attempts, (attempt + 1) as u32);
            db.commit_parse(&job, Err(("暂时失败".into(), false)), now).unwrap();
        }
        assert!(db.claim_parse_job("test-bot", 1000).unwrap().is_none());
        assert_eq!(db.record(&record.id).unwrap().status, RecordStatus::Failed);
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 1);
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = queued_record(&db);
        db.connection.execute("UPDATE parse_jobs SET attempts=3", []).unwrap();
        assert!(db.claim_parse_job("test-bot", 1).unwrap().is_none());
        assert_eq!(db.record(&record.id).unwrap().status, RecordStatus::Failed);
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 1);
    }

    #[test]
    fn claim_identity_cancellation_and_account_isolation() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = queued_record(&db);
        assert!(db.claim_parse_job("other-account", 1).unwrap().is_none());
        let old = db.claim_parse_job("test-bot", 1).unwrap().unwrap();
        db.release_parse_claims().unwrap();
        let new = db.claim_parse_job("test-bot", 1).unwrap().unwrap();
        assert_eq!(new.attempts, 1, "cancellation does not consume retries");
        assert_eq!(new.reference_date, old.reference_date, "retry date stays fixed");
        assert_ne!(new.claim_token, old.claim_token);
        assert!(!db.commit_parse(&old, Ok((record.clone(), "旧结果".into())), 2).unwrap());
        assert!(db.commit_parse(&new, Ok((record, "新结果".into())), 2).unwrap());
        assert!(!db.commit_parse(&new, Err(("重复结果".into(), true)), 3).unwrap());
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 1);
    }

    #[test]
    fn parse_commit_rolls_back_all_writes_if_outbox_fails() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let original = queued_record(&db);
        let job = db.claim_parse_job("test-bot", 1).unwrap().unwrap();
        db.connection.execute_batch("CREATE TRIGGER reject_reply BEFORE INSERT ON wechat_outbox BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let mut updated = original.clone(); updated.title = "不能半保存".into();
        assert!(db.commit_parse(&job, Ok((updated.clone(), "结果".into())), 2).is_err());
        assert_eq!(db.record(&original.id).unwrap(), original);
        db.connection.execute_batch("DROP TRIGGER reject_reply;").unwrap();
        assert!(db.commit_parse(&job, Ok((updated, "结果".into())), 3).unwrap());
    }

    #[test]
    fn old_creation_does_not_shorten_archive_retention() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut record = sample_record(); record.created_at = "2000-01-01T00:00:00Z".into();
        db.insert_record(&record).unwrap();
        let archived = db.set_archived(&record.id, true).unwrap();
        assert_eq!(db.set_archived(&record.id, true).unwrap().archived_at, archived.archived_at);
        assert_eq!(db.auto_clear_archived("2026-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(db.auto_clear_archived(archived.archived_at.as_deref().unwrap()).unwrap(), 0, "strict retention boundary");
        assert!(db.set_archived(&record.id, false).unwrap().archived_at.is_none());
        assert!(db.set_archived(&record.id, true).unwrap().archived_at.is_some());
    }

    #[test]
    fn v5_archives_get_a_conservative_one_time_migration() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE records(id TEXT PRIMARY KEY,created_at TEXT NOT NULL,data TEXT NOT NULL); PRAGMA user_version=5;").unwrap();
        let mut record = sample_record(); record.status = RecordStatus::Archived; record.created_at = "2000-01-01T00:00:00Z".into();
        connection.execute("INSERT INTO records VALUES (?1,?2,?3)", params![record.id,record.created_at,serde_json::to_string(&record).unwrap()]).unwrap();
        let db = Database::initialize(connection).unwrap();
        let archived_at = db.record(&record.id).unwrap().archived_at;
        assert!(archived_at.is_some());
        assert_eq!(db.auto_clear_archived("2026-01-01T00:00:00Z").unwrap(), 0);
        let db = Database::initialize(db.connection).unwrap();
        assert_eq!(db.record(&record.id).unwrap().archived_at, archived_at);
    }

    #[test]
    fn migration_rolls_back_columns_and_version_when_records_are_corrupt() {
        let path = std::env::temp_dir().join(format!("shijian-migration-{}.sqlite3", uuid::Uuid::new_v4()));
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE records(id TEXT PRIMARY KEY,created_at TEXT NOT NULL,data TEXT NOT NULL); INSERT INTO records VALUES ('bad','2020-01-01','corrupt'); PRAGMA user_version=5;").unwrap();
        assert!(Database::initialize(connection).is_err());
        let connection = Connection::open(&path).unwrap();
        let version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, 5);
        assert!(connection.prepare("SELECT revision FROM records").is_err());
        drop(connection);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unreadable_records_are_not_silently_treated_as_deleted_jobs() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = queued_record(&db);
        db.connection.execute("UPDATE records SET data='corrupt' WHERE id=?1", [&record.id]).unwrap();
        assert!(db.claim_parse_job("test-bot", 1).is_err());
        let count: i64 = db.connection.query_row("SELECT COUNT(*) FROM parse_jobs", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn source_date_falls_back_for_invalid_or_future_timestamps() {
        let fallback = "2026-10-25T10:00:00Z";
        for message in [serde_json::json!({}), serde_json::json!({"create_time_ms":0}), serde_json::json!({"create_time_ms":i64::MAX})] {
            assert_eq!(message_reference_date(&message, fallback).unwrap(), reference_date(fallback).unwrap());
        }
        let sent = chrono::DateTime::parse_from_rfc3339("2026-10-24T10:00:00Z").unwrap();
        assert_eq!(message_reference_date(&serde_json::json!({"create_time_ms":sent.timestamp_millis()}), fallback).unwrap(), sent.with_timezone(&chrono::Local).date_naive().to_string());
    }

    fn session() -> Session {
        Session {
            token: "not-a-real-token".into(),
            bot_id: "test-bot".into(),
            user_id: "owner".into(),
            base_url: crate::wechat_client::LOGIN_BASE.into(),
        }
    }
    fn incoming(id: u64) -> Value {
        serde_json::json!({"message_id":id,"message_type":1,"message_state":2,"from_user_id":"owner","to_user_id":"test-bot","context_token":"test-context-not-real","item_list":[{"type":1,"text_item":{"text":"明天买牛奶"}}]})
    }
    fn batch(messages: Vec<Value>, cursor: &str) -> Value {
        serde_json::json!({"ret":0,"msgs":messages,"get_updates_buf":cursor})
    }

    #[test]
    #[cfg(windows)]
    fn duplicate_batches_and_u64_ids_never_duplicate_records_or_receipts() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let a = incoming(18_446_744_073_709_551_614);
        let b = incoming(18_446_744_073_709_551_615);
        assert_ne!(message_key(&a), message_key(&b));
        assert_eq!(
            db.ingest_wechat(&session(), &batch(vec![a.clone(), b.clone()], "cursor-1"))
                .unwrap(),
            2
        );
        let first = db.next_reply("test-bot").unwrap().unwrap().client_id;
        assert_eq!(
            db.ingest_wechat(&session(), &batch(vec![a, b], "cursor-2"))
                .unwrap(),
            0
        );
        assert_eq!(db.records().unwrap().len(), 2);
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 2);
        assert_eq!(db.next_reply("test-bot").unwrap().unwrap().client_id, first);
        assert_eq!(db.wechat_cursor("test-bot").unwrap(), "cursor-2");
        assert_eq!(db.wechat_cursor("other-bot").unwrap(), "");
        let export = serde_json::to_string(&db.records().unwrap()).unwrap();
        assert!(!export.contains("test-context-not-real"));
        assert!(!export.contains("not-a-real-token"));
        assert!(!export.contains("context_token"));
    }

    #[test]
    #[cfg(windows)]
    fn failed_batch_rolls_back_records_seen_receipts_and_cursor() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        db.ingest_wechat(&session(), &batch(vec![], "previous"))
            .unwrap();
        let mut invalid = incoming(2);
        invalid["context_token"] = Value::String("x".repeat(65_537));
        assert!(db
            .ingest_wechat(
                &session(),
                &batch(vec![incoming(1), invalid], "not-committed")
            )
            .is_err());
        assert!(db.records().unwrap().is_empty());
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 0);
        assert_eq!(db.wechat_cursor("test-bot").unwrap(), "previous");
        assert_eq!(
            db.ingest_wechat(&session(), &batch(vec![incoming(1)], "committed"))
                .unwrap(),
            1
        );
    }

    #[test]
    #[cfg(windows)]
    fn only_owner_complete_private_text_is_saved() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut foreign = incoming(1);
        foreign["from_user_id"] = Value::String("stranger".into());
        let mut group = incoming(2);
        group["group_id"] = Value::String("group".into());
        let mut bot = incoming(3);
        bot["message_type"] = Value::from(2);
        let mut incomplete = incoming(4);
        incomplete["message_state"] = Value::from(1);
        let mut empty = incoming(5);
        empty["item_list"][0]["text_item"]["text"] = Value::String(" ".into());
        assert_eq!(
            db.ingest_wechat(
                &session(),
                &batch(vec![foreign, group, bot, incomplete, empty], "cursor")
            )
            .unwrap(),
            0
        );
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 0);
        assert_eq!(
            db.ingest_wechat(&session(), &batch(vec![incoming(4)], "cursor-next"))
                .unwrap(),
            1
        );
    }

    #[test]
    #[cfg(windows)]
    fn mixed_media_is_rejected_without_a_false_saved_receipt() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut mixed = incoming(1);
        mixed["item_list"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"type":2,"image_item":{}}));
        let mut long = incoming(2);
        long["item_list"][0]["text_item"]["text"] = Value::String("文".repeat(50_001));
        assert_eq!(
            db.ingest_wechat(&session(), &batch(vec![mixed, long], "next"))
                .unwrap(),
            0
        );
        assert!(db.records().unwrap().is_empty());
        assert!(db
            .next_reply("test-bot")
            .unwrap()
            .unwrap()
            .reply
            .contains("未保存"));
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 2);
    }

    #[test]
    #[cfg(windows)]
    fn title_is_bounded_but_original_text_is_preserved() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut message = incoming(1);
        let original = "文".repeat(600);
        message["item_list"][0]["text_item"]["text"] = Value::String(original.clone());
        db.ingest_wechat(&session(), &batch(vec![message], "next"))
            .unwrap();
        let record = db.records().unwrap().remove(0);
        assert_eq!(record.title.chars().count(), 500);
        assert_eq!(record.original_text, original);
        assert_eq!(record.source, RecordSource::Wechat);
        assert!(record.due_date.is_none());
    }

    #[test]
    #[cfg(windows)]
    fn outbox_retries_are_bounded_and_preserve_client_id() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        db.ingest_wechat(&session(), &batch(vec![incoming(1)], "next"))
            .unwrap();
        let job = db.next_reply("test-bot").unwrap().unwrap();
        assert!(!job.context.contains("test-context-not-real"));
        assert_eq!(
            crate::wechat_secrets::unprotect_context(&job.context).unwrap(),
            "test-context-not-real"
        );
        for _ in 0..3 {
            db.connection.execute("UPDATE wechat_outbox SET next_attempt=0 WHERE client_id=?1", [&job.client_id]).unwrap();
            assert!(db.start_reply_attempt(&job.client_id).unwrap());
            db.finish_reply(&job.client_id, false, false).unwrap();
        }
        assert_eq!(db.wechat_stats("test-bot").unwrap().failed, 1);
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 0);
        db.retry_replies("test-bot").unwrap();
        let retry = db.next_reply("test-bot").unwrap().unwrap();
        assert_eq!(retry.client_id, job.client_id);
        assert_eq!(retry.attempts, 0);
        db.start_reply_attempt(&retry.client_id).unwrap();
        db.finish_reply(&retry.client_id, true, false).unwrap();
        assert_eq!(db.wechat_stats("test-bot").unwrap().pending, 0);
        let context: String = db
            .connection
            .query_row(
                "SELECT context FROM wechat_outbox WHERE client_id=?1",
                [retry.client_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(context.is_empty());
    }

    #[test]
    fn missing_context_is_visible_as_failed_not_sent() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut message = incoming(1);
        message.as_object_mut().unwrap().remove("context_token");
        db.ingest_wechat(&session(), &batch(vec![message], "next"))
            .unwrap();
        assert_eq!(db.records().unwrap().len(), 1);
        assert_eq!(db.wechat_stats("test-bot").unwrap().failed, 1);
        assert!(db.next_reply("test-bot").unwrap().is_none());
        assert!(db.retry_replies("test-bot").is_err());
        assert_eq!(db.wechat_stats("test-bot").unwrap().failed, 1);
        db.clear_wechat_contexts().unwrap();
        assert_eq!(db.records().unwrap().len(), 1);
        assert_eq!(db.wechat_stats("test-bot").unwrap().failed, 0);
    }

    #[test]
    fn successful_empty_heartbeats_advance_only_valid_nonempty_cursors() {
        let mut db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        assert_eq!(
            db.ingest_wechat(
                &session(),
                &serde_json::json!({"ret":0,"get_updates_buf":"heartbeat"})
            )
            .unwrap(),
            0
        );
        assert_eq!(db.wechat_cursor("test-bot").unwrap(), "heartbeat");
        db.ingest_wechat(&session(), &serde_json::json!({"ret":0}))
            .unwrap();
        assert_eq!(db.wechat_cursor("test-bot").unwrap(), "heartbeat");
        for response in [
            serde_json::json!({"msgs":null,"ret":0,"get_updates_buf":"bad"}),
            serde_json::json!({"ret":1,"get_updates_buf":"bad"}),
            serde_json::json!({"ret":0,"get_updates_buf":123}),
            serde_json::json!({}),
        ] {
            assert!(db.ingest_wechat(&session(), &response).is_err());
            assert_eq!(db.wechat_cursor("test-bot").unwrap(), "heartbeat");
        }
    }

    #[test]
    fn schema_upgrade_preserves_v1_data_and_rejects_future_versions() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE settings(id INTEGER PRIMARY KEY,data TEXT NOT NULL); CREATE TABLE records(id TEXT PRIMARY KEY,created_at TEXT,data TEXT); PRAGMA user_version=1;").unwrap();
        let settings = Settings {
            paused: true,
            ..Settings::default()
        };
        connection
            .execute(
                "INSERT INTO settings(id,data) VALUES(1,?1)",
                [serde_json::to_string(&settings).unwrap()],
            )
            .unwrap();
        let db = Database::initialize(connection).unwrap();
        assert!(db.settings().unwrap().paused);
        let version: u32 = db
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 7);
        let future = Connection::open_in_memory().unwrap();
        future.execute_batch("PRAGMA user_version=8;").unwrap();
        assert!(Database::initialize(future).is_err());
    }

    #[test]
    fn legacy_unlock_reminder_mode_migrates_to_every_unlock() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        for (mode, expected) in [("unlock", UnlockReminder::EveryUnlock), ("daily", UnlockReminder::FirstOfDay)] {
            let legacy = format!(r#"{{"modelBaseUrl":"","modelName":"","destination":"local","destinationLabel":"本地收件箱","paused":false,"closeBehavior":"hide","retentionDays":30,"reminderMode":"{mode}"}}"#);
            db.connection.execute("UPDATE settings SET data = ?1 WHERE id = 1", [legacy]).unwrap();
            let settings = db.settings().unwrap();
            assert_eq!(settings.unlock_reminder, expected);
            assert!(settings.remind_on_unlock && settings.remind_on_startup);
            db.save_settings(&settings).unwrap();
            assert_eq!(db.settings().unwrap().unlock_reminder, expected, "迁移后保存不会再被覆盖");
        }
    }

    #[test]
    fn settings_and_records_roundtrip() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        assert_eq!(db.settings().unwrap(), Settings::default());
        assert!(db.records().unwrap().is_empty());
        let settings = Settings {
            paused: true,
            ..Settings::default()
        };
        db.save_settings(&settings).unwrap();
        assert_eq!(db.settings().unwrap(), settings);
        let record = InboxRecord {
            id: uuid::Uuid::new_v4().to_string(),
            title: "买牛奶".into(),
            original_text: "明天下班买牛奶".into(),
            status: RecordStatus::Saved,
            source: RecordSource::Local,
            created_at: "2026-10-23T12:00:00Z".into(),
            due_date: Some("2026-10-24".into()),
            priority: Priority::Normal,
            error: None,
            is_demo: false,
            status_before_archive: None,
            archived_at: None,
            due_time: None,
        };
        db.insert_record(&record).unwrap();
        assert_eq!(db.records().unwrap(), vec![record.clone()]);
        assert!(db.insert_record(&record).is_err());
        assert_eq!(
            db.set_archived(&record.id, true).unwrap().status,
            RecordStatus::Archived
        );
        assert_eq!(db.set_archived(&record.id, false).unwrap(), record);
        assert!(db.record("missing").is_err());
        assert!(db.set_archived("missing", true).is_err());
    }

    #[test]
    fn archived_records_restore_their_previous_status() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut completed = sample_record();
        completed.status = RecordStatus::Completed;
        db.insert_record(&completed).unwrap();
        let archived = db.set_archived(&completed.id, true).unwrap();
        assert_eq!(archived.status, RecordStatus::Archived);
        assert_eq!(archived.status_before_archive, Some(RecordStatus::Completed));
        let restored = db.set_archived(&completed.id, false).unwrap();
        assert_eq!(restored.status, RecordStatus::Completed);
        assert_eq!(restored.status_before_archive, None);

        // 待确认记录恢复时保留状态和错误说明，否则"待确认"失去解释。
        let mut review = sample_record();
        review.status = RecordStatus::NeedsReview;
        review.error = Some("缺少出发日期".into());
        db.insert_record(&review).unwrap();
        db.set_archived(&review.id, true).unwrap();
        let restored_review = db.set_archived(&review.id, false).unwrap();
        assert_eq!(restored_review.status, RecordStatus::NeedsReview);
        assert_eq!(restored_review.error.as_deref(), Some("缺少出发日期"));

        // 重复归档不覆盖最初保存的状态。
        db.set_archived(&completed.id, true).unwrap();
        let twice = db.set_archived(&completed.id, true).unwrap();
        assert_eq!(twice.status_before_archive, Some(RecordStatus::Completed));
        assert_eq!(
            db.set_archived(&completed.id, false).unwrap().status,
            RecordStatus::Completed
        );
    }

    #[test]
    fn complete_reopen_and_permanent_delete() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut record = sample_record();
        record.error = Some("旧错误".into());
        db.insert_record(&record).unwrap();
        let completed = db.set_completed(&record.id, true).unwrap();
        assert_eq!(completed.status, RecordStatus::Completed);
        assert!(completed.error.is_none());
        let reopened = db.set_completed(&record.id, false).unwrap();
        assert_eq!(reopened.status, RecordStatus::Saved);
        db.mark_reminded(&record.id, "2026-10-24", date(2026, 10, 24), 1_800_000_000).unwrap();
        assert_eq!(db.last_reminder(&record.id, "2026-10-24").unwrap().map(|(_, at)| at), Some(1_800_000_000));
        db.delete_record(&record.id).unwrap();
        assert!(db.record(&record.id).is_err());
        assert!(db.delete_record(&record.id).is_err());
        // 删除记录必须连带清理提醒日志，避免残留阻止后续同 ID 复测。
        let remaining: usize = db
            .connection
            .query_row("SELECT COUNT(*) FROM reminder_log", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn auto_clear_only_removes_archived_records_past_retention() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let mut old_archived = sample_record();
        old_archived.created_at = "2020-01-01T00:00:00.000Z".into();
        old_archived.status = RecordStatus::Archived;
        old_archived.archived_at = Some(old_archived.created_at.clone());
        let mut old_active = sample_record();
        old_active.created_at = "2020-02-01T00:00:00.000Z".into();
        let mut new_archived = sample_record();
        new_archived.status = RecordStatus::Archived;
        new_archived.archived_at = Some(new_archived.created_at.clone());
        for record in [&old_archived, &old_active, &new_archived] {
            db.insert_record(record).unwrap();
        }
        let cleared = db.auto_clear_archived("2026-01-01T00:00:00.000Z").unwrap();
        assert_eq!(cleared, 1);
        let remaining = db.records().unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(remaining.iter().all(|r| r.id != old_archived.id));
    }

    #[test]
    fn reminder_log_keeps_only_the_latest_per_due_date() {
        let db = Database::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let record = sample_record();
        db.insert_record(&record).unwrap();
        db.mark_reminded(&record.id, "2026-10-24", date(2026, 10, 24), 1_000).unwrap();
        assert_eq!(
            db.last_reminder(&record.id, "2026-10-24").unwrap(),
            Some(("2026-10-24".into(), 1_000))
        );
        // 同一 (记录, 截止日) 再次提醒直接覆盖，不报冲突也不留旧行。
        db.mark_reminded(&record.id, "2026-10-24", date(2026, 10, 25), 2_000).unwrap();
        assert_eq!(
            db.last_reminder(&record.id, "2026-10-24").unwrap(),
            Some(("2026-10-25".into(), 2_000))
        );
        // 换截止日期是新的一行，提醒独立去重。
        db.mark_reminded(&record.id, "2026-10-26", date(2026, 10, 25), 2_000).unwrap();
        assert_eq!(
            db.last_reminder(&record.id, "2026-10-26").unwrap().map(|(_, at)| at),
            Some(2_000)
        );
        assert!(db.last_reminder(&record.id, "2026-10-30").unwrap().is_none());
        assert_eq!(db.prune_reminder_log("2026-10-25").unwrap(), 0, "没有更早的记录");
        // 更早的本地日期按天清理，最近的不受影响。
        db.mark_reminded(&record.id, "2026-10-20", date(2026, 10, 1), 500).unwrap();
        assert_eq!(db.prune_reminder_log("2026-10-25").unwrap(), 1);
        assert_eq!(db.last_reminder(&record.id, "2026-10-20").unwrap(), None);
        assert!(db.last_reminder(&record.id, "2026-10-24").unwrap().is_some());
    }

    #[test]
    fn v3_reminder_log_is_rebuilt_without_losing_records() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE settings(id INTEGER PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE records(id TEXT PRIMARY KEY, created_at TEXT, data TEXT NOT NULL);
             CREATE TABLE reminder_log(record_id TEXT NOT NULL, due_date TEXT NOT NULL, sent_on TEXT NOT NULL, PRIMARY KEY(record_id, due_date, sent_on));
             PRAGMA user_version=3;",
        ).unwrap();
        let record = InboxRecord {
            id: "11111111-1111-4111-8111-111111111111".into(),
            title: "旧记录".into(),
            original_text: "旧原始文本".into(),
            status: RecordStatus::Saved,
            source: RecordSource::Local,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            due_date: Some("2026-10-24".into()),
            priority: Priority::Normal,
            error: None,
            is_demo: false,
            status_before_archive: None,
            archived_at: None,
            due_time: None,
        };
        connection.execute(
            "INSERT INTO records(id, created_at, data) VALUES (?1, ?2, ?3)",
            params![record.id, record.created_at, serde_json::to_string(&record).unwrap()],
        ).unwrap();
        connection.execute(
            "INSERT INTO reminder_log(record_id, due_date, sent_on) VALUES (?1, '2026-10-24', '2026-10-24')",
            [&record.id],
        ).unwrap();
        let db = Database::initialize(connection).unwrap();
        assert_eq!(db.records().unwrap(), vec![record.clone()]);
        assert_eq!(
            db.claim_parse_job("test-bot", Utc::now().timestamp()).unwrap().map(|job| job.record_id),
            None,
            "重建 reminder_log 不应影响解析队列表"
        );
        // 旧表已按新结构重建：同 (记录, 截止日) 覆盖写而不是冲突。
        db.mark_reminded(&record.id, "2026-10-24", date(2026, 10, 25), 5_000).unwrap();
        db.mark_reminded(&record.id, "2026-10-24", date(2026, 10, 26), 6_000).unwrap();
        assert_eq!(
            db.last_reminder(&record.id, "2026-10-24").unwrap().map(|(_, at)| at),
            Some(6_000)
        );
        let version: u32 = db
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 7);
    }

    fn sample_record() -> InboxRecord {
        InboxRecord {
            id: uuid::Uuid::new_v4().to_string(),
            title: "买牛奶".into(),
            original_text: "明天下班买牛奶".into(),
            status: RecordStatus::Saved,
            source: RecordSource::Local,
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            due_date: None,
            priority: Priority::Normal,
            error: None,
            is_demo: false,
            status_before_archive: None,
            archived_at: None,
            due_time: None,
        }
    }

    fn date(year: i32, month: u32, day: u32) -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn disk_database_persists() {
        let path =
            std::env::temp_dir().join(format!("shijian-test-{}.sqlite3", uuid::Uuid::new_v4()));
        {
            let db = Database::open(&path).unwrap();
            db.save_settings(&Settings {
                retention_days: 7,
                ..Settings::default()
            })
            .unwrap();
        }
        {
            let db = Database::open(&path).unwrap();
            assert_eq!(db.settings().unwrap().retention_days, 7);
        }
        std::fs::remove_file(path).unwrap();
    }
}
