use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::Manager;
use url::Url;

use crate::{qlogin::QLoginState, qzone};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveProgress {
    status: &'static str,
    pages: u32,
    fetched: u64,
    saved: u64,
    skipped: u32,
    message: String,
    retry_at: Option<i64>,
    batch_retry: Option<BatchRetryProgress>,
}

/// 批量重试异常跳过记录时的实时进度。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRetryProgress {
    current: u32,
    total: u32,
    recovered: u32,
    failed: u32,
    recovered_records: u64,
}

impl Default for ArchiveProgress {
    fn default() -> Self {
        Self {
            status: "idle",
            pages: 0,
            fetched: 0,
            saved: 0,
            skipped: 0,
            message: "尚未开始归档".into(),
            retry_at: None,
            batch_retry: None,
        }
    }
}

pub struct ArchiveState {
    progress: Mutex<ArchiveProgress>,
    cancel: AtomicBool,
    batch_retrying: AtomicBool,
    batch_cancel: AtomicBool,
    image_downloads: tokio::sync::Semaphore,
}

impl ArchiveState {
    pub fn new() -> Self {
        Self {
            progress: Mutex::new(ArchiveProgress::default()),
            cancel: AtomicBool::new(false),
            batch_retrying: AtomicBool::new(false),
            batch_cancel: AtomicBool::new(false),
            image_downloads: tokio::sync::Semaphore::new(4),
        }
    }

    /// 相册备份等兄弟模块用于避免与动态归档同时打接口。
    pub(crate) fn is_feed_archive_running(&self) -> bool {
        self.progress
            .lock()
            .map(|progress| progress.status == "running")
            .unwrap_or(false)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveItem {
    #[serde(skip)]
    owner_uin: String,
    pub(crate) id: i64,
    cell_id: String,
    published_at: i64,
    content: Option<String>,
    author_uin: Option<String>,
    author_name: Option<String>,
    picture_urls: Vec<String>,
    video_url: Option<String>,
    video_urls: Vec<String>,
    video_cover_url: Option<String>,
    like_count: i64,
    comment_count: i64,
    likes: Vec<LikeUser>,
    comments: Vec<ArchiveComment>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveComment {
    #[serde(skip)]
    comment_id: Option<String>,
    uin: Option<String>,
    nickname: Option<String>,
    content: String,
    created_at: i64,
    replies: Vec<ArchiveReply>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveReply {
    uin: Option<String>,
    nickname: Option<String>,
    reply_to_uin: Option<String>,
    reply_to_nickname: Option<String>,
    content: String,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LikeUser {
    uin: Option<String>,
    nickname: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Interactor {
    uin: String,
    nickname: String,
    likes: u64,
    comments: u64,
    total: u64,
    last_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOverview {
    dynamics: u64,
    pictures: u64,
    comments: u64,
    likes: u64,
    albums: u64,
    album_photos_backed: u64,
    database_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveHealth {
    healthy: bool,
    integrity: String,
    schema_version: i64,
    dynamics: u64,
    interactions: u64,
    pending_skips: u64,
    database_bytes: u64,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionRank {
    uin: String,
    nickname: String,
    interactions: u64,
    likes: u64,
    comments: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveMediaItem {
    key: String,
    dynamic_id: i64,
    media_type: &'static str,
    picture_index: Option<usize>,
    url: String,
    cover_url: Option<String>,
    published_at: i64,
    author_uin: Option<String>,
    author_name: Option<String>,
    content: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveMediaPage {
    items: Vec<ArchiveMediaItem>,
    total: usize,
    years: Vec<i32>,
}

struct ParsedFeed {
    feed_key: String,
    cell_id: Option<String>,
    event_type: i64,
    event_time: i64,
    title: Option<String>,
    content: Option<String>,
    event_summary: Option<String>,
    actor_uin: Option<String>,
    actor_name: Option<String>,
    original_author_uin: Option<String>,
    original_author_name: Option<String>,
    picture_count: i64,
    pictures_json: Option<String>,
    video_json: Option<String>,
    comments_json: Option<String>,
    raw_json: String,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSkipItem {
    id: i64,
    page_number: u32,
    cursor_offset: i64,
    offset_advance: i64,
    base_time: i64,
    error: String,
    skipped_at: i64,
    retry_count: u32,
    last_retry_at: Option<i64>,
    resolved_at: Option<i64>,
    recovered_records: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSkipRetryResult {
    success: bool,
    message: String,
    recovered_records: u64,
}

pub(crate) fn stable_feed_hash(value: &Value) -> u64 {
    // FNV-1a keeps fallback keys deterministic without adding a hashing dependency.
    value
        .to_string()
        .bytes()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

fn archive_page_delay_ms(interval_ms: u64) -> u64 {
    let interval_ms = interval_ms.clamp(2_000, 30_000);
    let jitter_range = (interval_ms / 4).max(1);
    let subsecond_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    interval_ms + subsecond_nanos % (jitter_range + 1)
}

pub(crate) fn database_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?;
    fs::create_dir_all(&dir).map_err(|error| format!("无法创建应用数据目录：{error}"))?;
    Ok(dir.join("qzone-archive.sqlite3"))
}

/// 数据库结构版本：仅在版本落后时执行一次建表与数据迁移；
/// 否则归档循环里每页多次打开数据库都会重复执行 DDL 与全表扫描，库越大越慢。
const DATABASE_SCHEMA_VERSION: i32 = 3;

pub(crate) fn open_database(app: &tauri::AppHandle) -> Result<Connection, String> {
    let mut connection = Connection::open(database_path(app)?)
        .map_err(|error| format!("无法打开归档数据库：{error}"))?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|error| format!("初始化归档数据库失败：{error}"))?;
    let schema_version: i32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("读取归档数据库版本失败：{error}"))?;
    if schema_version >= DATABASE_SCHEMA_VERSION {
        init_search_index(&mut connection)?;
        init_global_search_index(&mut connection)?;
        return Ok(connection);
    }
    connection
        .execute_batch(
            "
         CREATE TABLE IF NOT EXISTS archive_feeds (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           owner_uin TEXT NOT NULL,
           feed_key TEXT NOT NULL,
           cell_id TEXT,
           event_type INTEGER NOT NULL DEFAULT 0,
           event_time INTEGER NOT NULL DEFAULT 0,
           title TEXT,
           content TEXT,
           event_summary TEXT,
           actor_uin TEXT,
           actor_name TEXT,
           original_author_uin TEXT,
           original_author_name TEXT,
           picture_count INTEGER NOT NULL DEFAULT 0,
           pictures_json TEXT,
           video_json TEXT,
           comments_json TEXT,
           raw_json TEXT NOT NULL,
           archived_at INTEGER NOT NULL,
           UNIQUE(owner_uin, feed_key)
         );
         CREATE INDEX IF NOT EXISTS idx_archive_feeds_owner_time
           ON archive_feeds(owner_uin, event_time DESC);
         CREATE INDEX IF NOT EXISTS idx_archive_feeds_dynamic_type
           ON archive_feeds(owner_uin, cell_id, event_type);
         CREATE TABLE IF NOT EXISTS archive_dynamics (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           owner_uin TEXT NOT NULL,
           cell_id TEXT NOT NULL,
           published_at INTEGER NOT NULL DEFAULT 0,
           content TEXT,
           author_uin TEXT,
           author_name TEXT,
           category TEXT NOT NULL DEFAULT '',
           pictures_json TEXT,
           video_json TEXT,
           raw_original_json TEXT NOT NULL,
           archived_at INTEGER NOT NULL,
           UNIQUE(owner_uin, cell_id)
         );
         CREATE INDEX IF NOT EXISTS idx_archive_dynamics_owner_time
           ON archive_dynamics(owner_uin, published_at DESC);
         CREATE TABLE IF NOT EXISTS archive_checkpoints (
           owner_uin TEXT PRIMARY KEY,
           attach_info TEXT NOT NULL,
           pages INTEGER NOT NULL DEFAULT 0,
           fetched INTEGER NOT NULL DEFAULT 0,
           saved INTEGER NOT NULL DEFAULT 0,
           updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS archive_rate_limits (
           owner_uin TEXT PRIMARY KEY,
           window_started_at INTEGER NOT NULL,
           requested_pages INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS archive_skips (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           owner_uin TEXT NOT NULL,
           cursor TEXT NOT NULL,
           resume_cursor TEXT NOT NULL,
           page_number INTEGER NOT NULL,
           cursor_offset INTEGER NOT NULL,
           offset_advance INTEGER NOT NULL,
           base_time INTEGER NOT NULL,
           error TEXT NOT NULL,
           skipped_at INTEGER NOT NULL,
           retry_count INTEGER NOT NULL DEFAULT 0,
           last_retry_at INTEGER,
           resolved_at INTEGER,
           recovered_records INTEGER NOT NULL DEFAULT 0,
           UNIQUE(owner_uin, cursor_offset, base_time)
         );
         CREATE TABLE IF NOT EXISTS archive_albums (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           owner_uin TEXT NOT NULL,
           album_id TEXT NOT NULL,
           name TEXT,
           description TEXT,
           cover_url TEXT,
           photo_total INTEGER NOT NULL DEFAULT 0,
           backed_count INTEGER NOT NULL DEFAULT 0,
           failed_count INTEGER NOT NULL DEFAULT 0,
           status TEXT NOT NULL DEFAULT 'pending',
           create_time INTEGER,
           modify_time INTEGER,
           archived_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           UNIQUE(owner_uin, album_id)
         );
         CREATE INDEX IF NOT EXISTS idx_archive_albums_owner
           ON archive_albums(owner_uin, modify_time DESC);
         CREATE TABLE IF NOT EXISTS archive_album_photos (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           owner_uin TEXT NOT NULL,
           album_id TEXT NOT NULL,
           pic_key TEXT NOT NULL,
           name TEXT,
           description TEXT,
           upload_time INTEGER,
           preview_url TEXT,
           urls_json TEXT NOT NULL,
           local_path TEXT,
           file_size INTEGER NOT NULL DEFAULT 0,
           status TEXT NOT NULL DEFAULT 'pending',
           error TEXT,
           archived_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           UNIQUE(owner_uin, album_id, pic_key)
         );
         CREATE INDEX IF NOT EXISTS idx_archive_album_photos_album
           ON archive_album_photos(owner_uin, album_id, upload_time);
         CREATE INDEX IF NOT EXISTS idx_archive_album_photos_status
           ON archive_album_photos(owner_uin, album_id, status);
         CREATE TABLE IF NOT EXISTS friend_contacts (
           owner_uin TEXT NOT NULL,
           uin TEXT NOT NULL,
           nickname TEXT,
           remark TEXT,
           archived_at INTEGER NOT NULL,
           UNIQUE(owner_uin, uin)
         );",
        )
        .map_err(|error| format!("初始化归档数据库失败：{error}"))?;
    if connection
        .prepare("SELECT pages,fetched,saved FROM archive_checkpoints LIMIT 0")
        .is_err()
    {
        connection
            .execute_batch(
                "ALTER TABLE archive_checkpoints ADD COLUMN pages INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE archive_checkpoints ADD COLUMN fetched INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE archive_checkpoints ADD COLUMN saved INTEGER NOT NULL DEFAULT 0;",
            )
            .map_err(|error| format!("升级归档续传统计失败：{error}"))?;
    }
    if connection
        .prepare("SELECT category FROM archive_dynamics LIMIT 0")
        .is_err()
    {
        connection
            .execute(
                "ALTER TABLE archive_dynamics ADD COLUMN category TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(|error| format!("升级归档分类失败：{error}"))?;
    }
    migrate_legacy_dynamics(&mut connection)?;
    migrate_dynamic_categories(&mut connection)?;
    init_search_index(&mut connection)?;
    init_global_search_index(&mut connection)?;
    connection
        .pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)
        .map_err(|error| format!("写入归档数据库版本失败：{error}"))?;
    Ok(connection)
}

/// 全文搜索索引：对 archive_dynamics 的正文与作者建立 FTS5 虚拟表，
/// 用触发器增量维护，首次建表时全量填充历史数据。
fn init_search_index(connection: &mut Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
         CREATE VIRTUAL TABLE IF NOT EXISTS archive_search USING fts5(
           content, author_name, cell_id, owner_uin,
           tokenize = 'unicode61'
         );
         CREATE TRIGGER IF NOT EXISTS archive_search_ai AFTER INSERT ON archive_dynamics BEGIN
           INSERT INTO archive_search(rowid, content, author_name, cell_id, owner_uin)
           VALUES (new.id, new.content, new.author_name, new.cell_id, new.owner_uin);
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_ad AFTER DELETE ON archive_dynamics BEGIN
           INSERT INTO archive_search(archive_search, rowid, content, author_name, cell_id, owner_uin)
           VALUES ('delete', old.id, old.content, old.author_name, old.cell_id, old.owner_uin);
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_au AFTER UPDATE ON archive_dynamics BEGIN
           INSERT INTO archive_search(archive_search, rowid, content, author_name, cell_id, owner_uin)
           VALUES ('delete', old.id, old.content, old.author_name, old.cell_id, old.owner_uin);
           INSERT INTO archive_search(rowid, content, author_name, cell_id, owner_uin)
           VALUES (new.id, new.content, new.author_name, new.cell_id, new.owner_uin);
         END;
         ",
        )
        .map_err(|error| format!("初始化全文搜索索引失败：{error}"))?;
    // 首次建表后，把已有动态全量写入索引。
    let has_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM archive_search", [], |row| row.get(0))
        .map_err(|error| format!("检查搜索索引失败：{error}"))?;
    if has_rows == 0 {
        connection
            .execute_batch(
                "INSERT INTO archive_search(rowid, content, author_name, cell_id, owner_uin)
                 SELECT id, content, author_name, cell_id, owner_uin FROM archive_dynamics;",
            )
            .map_err(|error| format!("填充搜索索引失败：{error}"))?;
    }
    Ok(())
}

fn init_global_search_index(connection: &mut Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "
         CREATE TABLE IF NOT EXISTS archive_search_documents (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           owner_uin TEXT NOT NULL,
           kind TEXT NOT NULL,
           ref_id INTEGER NOT NULL,
           cell_id TEXT,
           title TEXT,
           body TEXT,
           author_name TEXT,
           created_at INTEGER NOT NULL DEFAULT 0,
           UNIQUE(owner_uin, kind, ref_id)
         );
         CREATE INDEX IF NOT EXISTS idx_archive_search_documents_owner
           ON archive_search_documents(owner_uin, created_at DESC);
         CREATE VIRTUAL TABLE IF NOT EXISTS archive_global_search USING fts5(
           title, body, author_name,
           owner_uin UNINDEXED, kind UNINDEXED, cell_id UNINDEXED,
           tokenize = 'unicode61'
         );
         CREATE TABLE IF NOT EXISTS archive_search_meta (
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL
         );
         CREATE TRIGGER IF NOT EXISTS archive_search_dynamics_ai AFTER INSERT ON archive_dynamics BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,body,author_name,created_at)
           VALUES (new.owner_uin,'dynamic',new.id,new.cell_id,new.content,new.author_name,new.published_at)
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET cell_id=excluded.cell_id,body=excluded.body,author_name=excluded.author_name,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='dynamic' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='dynamic' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_dynamics_au AFTER UPDATE ON archive_dynamics BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,body,author_name,created_at)
           VALUES (new.owner_uin,'dynamic',new.id,new.cell_id,new.content,new.author_name,new.published_at)
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET cell_id=excluded.cell_id,body=excluded.body,author_name=excluded.author_name,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='dynamic' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='dynamic' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_dynamics_ad AFTER DELETE ON archive_dynamics BEGIN
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='dynamic' AND ref_id=old.id);
           DELETE FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='dynamic' AND ref_id=old.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_feeds_ai AFTER INSERT ON archive_feeds
           WHEN new.event_type IN (2,311) BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,body,author_name,created_at)
           VALUES (new.owner_uin,'comment',new.id,new.cell_id,COALESCE(new.content,new.event_summary),new.actor_name,new.event_time)
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET cell_id=excluded.cell_id,body=excluded.body,author_name=excluded.author_name,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='comment' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='comment' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_feeds_au_old AFTER UPDATE ON archive_feeds
           WHEN old.event_type IN (2,311) BEGIN
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='comment' AND ref_id=old.id);
           DELETE FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='comment' AND ref_id=old.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_feeds_au_new AFTER UPDATE ON archive_feeds
           WHEN new.event_type IN (2,311) BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,body,author_name,created_at)
           VALUES (new.owner_uin,'comment',new.id,new.cell_id,COALESCE(new.content,new.event_summary),new.actor_name,new.event_time)
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET cell_id=excluded.cell_id,body=excluded.body,author_name=excluded.author_name,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='comment' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='comment' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_feeds_ad AFTER DELETE ON archive_feeds
           WHEN old.event_type IN (2,311) BEGIN
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='comment' AND ref_id=old.id);
           DELETE FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='comment' AND ref_id=old.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_albums_ai AFTER INSERT ON archive_albums BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,title,body,created_at)
           VALUES (new.owner_uin,'album',new.id,new.name,new.description,COALESCE(new.modify_time,new.create_time,0))
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET title=excluded.title,body=excluded.body,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='album' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='album' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_albums_au AFTER UPDATE ON archive_albums BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,title,body,created_at)
           VALUES (new.owner_uin,'album',new.id,new.name,new.description,COALESCE(new.modify_time,new.create_time,0))
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET title=excluded.title,body=excluded.body,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='album' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='album' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_albums_ad AFTER DELETE ON archive_albums BEGIN
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='album' AND ref_id=old.id);
           DELETE FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='album' AND ref_id=old.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_photos_ai AFTER INSERT ON archive_album_photos BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,title,body,created_at)
           VALUES (new.owner_uin,'photo',new.id,new.album_id,new.name,new.description,COALESCE(new.upload_time,0))
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET cell_id=excluded.cell_id,title=excluded.title,body=excluded.body,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='photo' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='photo' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_photos_au AFTER UPDATE ON archive_album_photos BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,title,body,created_at)
           VALUES (new.owner_uin,'photo',new.id,new.album_id,new.name,new.description,COALESCE(new.upload_time,0))
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET cell_id=excluded.cell_id,title=excluded.title,body=excluded.body,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='photo' AND ref_id=new.id);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='photo' AND ref_id=new.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_photos_ad AFTER DELETE ON archive_album_photos BEGIN
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='photo' AND ref_id=old.id);
           DELETE FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='photo' AND ref_id=old.id;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_contacts_ai AFTER INSERT ON friend_contacts BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,title,body,author_name,created_at)
           VALUES (new.owner_uin,'contact',new.rowid,new.nickname,new.remark,new.nickname,new.archived_at)
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET title=excluded.title,body=excluded.body,author_name=excluded.author_name,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='contact' AND ref_id=new.rowid);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='contact' AND ref_id=new.rowid;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_contacts_au AFTER UPDATE ON friend_contacts BEGIN
           INSERT INTO archive_search_documents(owner_uin,kind,ref_id,title,body,author_name,created_at)
           VALUES (new.owner_uin,'contact',new.rowid,new.nickname,new.remark,new.nickname,new.archived_at)
           ON CONFLICT(owner_uin,kind,ref_id) DO UPDATE SET title=excluded.title,body=excluded.body,author_name=excluded.author_name,created_at=excluded.created_at;
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=new.owner_uin AND kind='contact' AND ref_id=new.rowid);
           INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
           SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents
           WHERE owner_uin=new.owner_uin AND kind='contact' AND ref_id=new.rowid;
         END;
         CREATE TRIGGER IF NOT EXISTS archive_search_contacts_ad AFTER DELETE ON friend_contacts BEGIN
           DELETE FROM archive_global_search WHERE rowid=(SELECT id FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='contact' AND ref_id=old.rowid);
           DELETE FROM archive_search_documents WHERE owner_uin=old.owner_uin AND kind='contact' AND ref_id=old.rowid;
         END;
         ",
        )
        .map_err(|error| format!("初始化全局搜索索引失败：{error}"))?;

    let version = connection
        .query_row(
            "SELECT value FROM archive_search_meta WHERE key='global-index-version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default();
    if version != "1" {
        connection
            .execute_batch(
                "DELETE FROM archive_global_search;
                 DELETE FROM archive_search_documents;
                 INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,body,author_name,created_at)
                   SELECT owner_uin,'dynamic',id,cell_id,content,author_name,published_at FROM archive_dynamics;
                 INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,body,author_name,created_at)
                   SELECT owner_uin,'comment',id,cell_id,COALESCE(content,event_summary),actor_name,event_time FROM archive_feeds WHERE event_type IN (2,311);
                 INSERT INTO archive_search_documents(owner_uin,kind,ref_id,title,body,created_at)
                   SELECT owner_uin,'album',id,name,description,COALESCE(modify_time,create_time,0) FROM archive_albums;
                 INSERT INTO archive_search_documents(owner_uin,kind,ref_id,cell_id,title,body,created_at)
                   SELECT owner_uin,'photo',id,album_id,name,description,COALESCE(upload_time,0) FROM archive_album_photos;
                 INSERT INTO archive_search_documents(owner_uin,kind,ref_id,title,body,author_name,created_at)
                   SELECT owner_uin,'contact',rowid,nickname,remark,nickname,archived_at FROM friend_contacts;
                 INSERT INTO archive_global_search(rowid,title,body,author_name,owner_uin,kind,cell_id)
                   SELECT id,title,body,author_name,owner_uin,kind,cell_id FROM archive_search_documents;
                 INSERT INTO archive_search_meta(key,value) VALUES ('global-index-version','1')
                   ON CONFLICT(key) DO UPDATE SET value=excluded.value;",
            )
            .map_err(|error| format!("构建全局搜索索引失败：{error}"))?;
    }
    Ok(())
}

fn migrate_dynamic_categories(connection: &mut Connection) -> Result<(), String> {
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_dynamics WHERE category=''",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查归档分类迁移状态失败：{error}"))?;
    if pending == 0 {
        return Ok(());
    }
    let feeds = {
        let mut statement = connection
            .prepare("SELECT owner_uin,raw_json FROM archive_feeds")
            .map_err(|error| format!("读取待分类归档失败：{error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("查询待分类归档失败：{error}"))?;
        collect_rows_logged(rows)
    };
    let transaction = connection
        .transaction()
        .map_err(|error| format!("开始归档分类迁移失败：{error}"))?;
    for (owner_uin, raw_json) in feeds {
        if let Ok(feed) = serde_json::from_str::<Value>(&raw_json) {
            save_original_dynamic(&transaction, &owner_uin, &feed)?;
        }
    }
    transaction.execute(
        "UPDATE archive_dynamics SET category=CASE WHEN author_uin=owner_uin THEN 'self' ELSE 'other' END WHERE category=''",
        [],
    ).map_err(|error| format!("补全归档分类失败：{error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("提交归档分类迁移失败：{error}"))
}

fn migrate_legacy_dynamics(connection: &mut Connection) -> Result<(), String> {
    let dynamic_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM archive_dynamics", [], |row| {
            row.get(0)
        })
        .map_err(|error| format!("检查原动态迁移状态失败：{error}"))?;
    if dynamic_count > 0 {
        return Ok(());
    }
    let legacy = {
        let mut statement = connection
            .prepare("SELECT owner_uin,raw_json FROM archive_feeds")
            .map_err(|error| format!("读取旧归档失败：{error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| format!("查询旧归档失败：{error}"))?;
        collect_rows_logged(rows)
    };
    if legacy.is_empty() {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| format!("开始旧归档迁移失败：{error}"))?;
    for (owner_uin, raw_json) in legacy {
        if let Ok(feed) = serde_json::from_str::<Value>(&raw_json) {
            save_original_dynamic(&transaction, &owner_uin, &feed)?;
        }
    }
    transaction
        .commit()
        .map_err(|error| format!("提交旧归档迁移失败：{error}"))
}

fn text_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|value| match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn parse_feed(feed: &Value) -> Result<ParsedFeed, String> {
    let cell_id = text_at(feed, "/original/cell_id/cellid");
    let event_time = feed
        .pointer("/comm/time")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let event_type = feed
        .pointer("/comm/subid")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let actor_uin = text_at(feed, "/userinfo/user/uin");
    let feed_key = text_at(feed, "/comm/feedskey")
        .or_else(|| text_at(feed, "/original/cell_comm/feedskey"))
        .or_else(|| {
            cell_id.as_ref().map(|id| {
                format!(
                    "{event_type}:{id}:{event_time}:{}",
                    actor_uin.as_deref().unwrap_or("unknown")
                )
            })
        })
        .unwrap_or_else(|| {
            format!(
                "fallback:{event_type}:{event_time}:{}:{:016x}",
                actor_uin.as_deref().unwrap_or("unknown"),
                stable_feed_hash(feed)
            )
        });
    let pictures = feed.pointer("/original/cell_pic");
    let picture_count = pictures
        .and_then(|value| value.pointer("/picdata/pic"))
        .and_then(Value::as_array)
        .map(|items| items.len() as i64)
        .unwrap_or(0);
    let video = feed
        .pointer("/original/cell_video")
        .filter(|value| !value.is_null());
    let comments = feed
        .pointer("/original/cell_comment")
        .filter(|value| !value.is_null());
    Ok(ParsedFeed {
        feed_key,
        cell_id,
        event_type,
        event_time,
        title: text_at(feed, "/title/title"),
        content: text_at(feed, "/original/cell_summary/summary"),
        event_summary: text_at(feed, "/summary/summary"),
        actor_uin,
        actor_name: text_at(feed, "/userinfo/user/nickname"),
        original_author_uin: text_at(feed, "/original/cell_userinfo/user/uin"),
        original_author_name: text_at(feed, "/original/cell_userinfo/user/nickname"),
        picture_count,
        pictures_json: pictures.map(Value::to_string),
        video_json: video.map(Value::to_string),
        comments_json: comments.map(Value::to_string),
        raw_json: feed.to_string(),
    })
}

fn save_feed_rows(
    transaction: &rusqlite::Transaction<'_>,
    owner_uin: &str,
    feeds: &[Value],
) -> Result<u64, String> {
    let mut saved = 0;
    for feed in feeds {
        save_original_dynamic(transaction, owner_uin, feed)?;
        let feed = parse_feed(feed)?;
        let changed = transaction.execute(
            "INSERT INTO archive_feeds
             (owner_uin, feed_key, cell_id, event_type, event_time, title, content, event_summary,
              actor_uin, actor_name, original_author_uin, original_author_name, picture_count,
              pictures_json, video_json, comments_json, raw_json, archived_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
             ON CONFLICT(owner_uin, feed_key) DO UPDATE SET
              cell_id=excluded.cell_id,event_type=excluded.event_type,event_time=excluded.event_time,
              title=excluded.title,content=excluded.content,event_summary=excluded.event_summary,
              actor_uin=excluded.actor_uin,actor_name=excluded.actor_name,
              original_author_uin=excluded.original_author_uin,original_author_name=excluded.original_author_name,
              picture_count=excluded.picture_count,pictures_json=excluded.pictures_json,
              video_json=excluded.video_json,comments_json=excluded.comments_json,
              raw_json=excluded.raw_json,archived_at=excluded.archived_at",
            params![owner_uin, feed.feed_key, feed.cell_id, feed.event_type, feed.event_time,
                feed.title, feed.content, feed.event_summary, feed.actor_uin, feed.actor_name,
                feed.original_author_uin, feed.original_author_name, feed.picture_count,
                feed.pictures_json, feed.video_json, feed.comments_json, feed.raw_json, now()],
        ).map_err(|error| format!("保存动态失败：{error}"))?;
        saved += changed as u64;
    }
    Ok(saved)
}

fn save_page(
    app: &tauri::AppHandle,
    owner_uin: &str,
    feeds: &[Value],
    next_cursor: Option<&str>,
    reset_checkpoint_stats: bool,
) -> Result<u64, String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("无法开始数据库事务：{error}"))?;
    let saved = save_feed_rows(&transaction, owner_uin, feeds)?;
    if let Some(cursor) = next_cursor {
        if reset_checkpoint_stats {
            transaction.execute(
                "INSERT INTO archive_checkpoints(owner_uin,attach_info,pages,fetched,saved,updated_at) VALUES (?1,?2,1,?3,?4,?5)
                 ON CONFLICT(owner_uin) DO UPDATE SET attach_info=excluded.attach_info,
                  pages=1,fetched=excluded.fetched,saved=excluded.saved,updated_at=excluded.updated_at",
                params![owner_uin, cursor, feeds.len() as u64, saved, now()],
            ).map_err(|error| format!("重置归档续传位置失败：{error}"))?;
        } else {
            transaction.execute(
                "INSERT INTO archive_checkpoints(owner_uin,attach_info,pages,fetched,saved,updated_at) VALUES (?1,?2,1,?3,?4,?5)
                 ON CONFLICT(owner_uin) DO UPDATE SET attach_info=excluded.attach_info,
                  pages=archive_checkpoints.pages+1,fetched=archive_checkpoints.fetched+excluded.fetched,
                  saved=archive_checkpoints.saved+excluded.saved,updated_at=excluded.updated_at",
                params![owner_uin, cursor, feeds.len() as u64, saved, now()],
            ).map_err(|error| format!("保存归档续传位置失败：{error}"))?;
        }
    } else {
        transaction
            .execute(
                "DELETE FROM archive_checkpoints WHERE owner_uin=?1",
                params![owner_uin],
            )
            .map_err(|error| format!("清除归档续传位置失败：{error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("提交归档事务失败：{error}"))?;
    Ok(saved)
}

fn save_retried_page(
    app: &tauri::AppHandle,
    owner_uin: &str,
    feeds: &[Value],
) -> Result<u64, String> {
    let mut connection = open_database(app)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("无法开始重试事务：{error}"))?;
    let saved = save_feed_rows(&transaction, owner_uin, feeds)?;
    transaction
        .commit()
        .map_err(|error| format!("提交重试事务失败：{error}"))?;
    Ok(saved)
}

struct ArchiveCheckpoint {
    cursor: String,
    pages: u32,
    fetched: u64,
    saved: u64,
    updated_at: i64,
}

const ARCHIVE_RATE_WINDOW_SECONDS: i64 = 10 * 60;
const ARCHIVE_RATE_PAGE_LIMIT: i64 = 300;
/// 续传游标最大存活时间：需覆盖一次完整的限流暂停窗口（10 分钟）并留有余量，
/// 否则每次限流后都会退化为从头全量重扫。
const ARCHIVE_CURSOR_MAX_AGE_SECONDS: i64 = 15 * 60;
const ARCHIVE_SKIP_MAX_OFFSET_ADVANCE: i64 = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FeedCursorDetails {
    offset: i64,
    base_time: i64,
    load_count: i64,
}

fn parse_query_pairs(value: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(value.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn serialize_query_pairs(pairs: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

fn pair_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn set_pair_value(pairs: &mut [(String, String)], key: &str, value: String) -> Result<(), String> {
    let pair = pairs
        .iter_mut()
        .find(|(candidate, _)| candidate == key)
        .ok_or_else(|| format!("分页游标缺少 {key}"))?;
    pair.1 = value;
    Ok(())
}

fn set_or_append_pair_value(pairs: &mut Vec<(String, String)>, key: &str, value: String) {
    if let Some(pair) = pairs.iter_mut().find(|(candidate, _)| candidate == key) {
        pair.1 = value;
    } else {
        pairs.push((key.to_owned(), value));
    }
}

fn parse_feed_cursor(cursor: &str) -> Result<FeedCursorDetails, String> {
    let outer = parse_query_pairs(cursor);
    let attach = pair_value(&outer, "att").ok_or("分页游标缺少 att")?;
    let attach = parse_query_pairs(attach);
    let backend = pair_value(&attach, "back_server_info").ok_or("分页游标缺少 back_server_info")?;
    let backend = parse_query_pairs(backend);
    let parse_number = |pairs: &[(String, String)], key: &str| {
        pair_value(pairs, key)
            .ok_or_else(|| format!("分页游标缺少 {key}"))?
            .parse::<i64>()
            .map_err(|_| format!("分页游标中的 {key} 不是有效数字"))
    };
    let load_count = pair_value(&outer, "loadcount")
        .or_else(|| pair_value(&attach, "loadcount"))
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| "分页游标中的 loadcount 不是有效数字".to_owned())
        })
        .transpose()?
        .unwrap_or(0);
    Ok(FeedCursorDetails {
        offset: parse_number(&backend, "offset")?,
        base_time: parse_number(&backend, "basetime")?,
        load_count,
    })
}

fn advance_feed_cursor(cursor: &str, offset_advance: i64) -> Result<String, String> {
    if offset_advance <= 0 {
        return Err("跳过偏移量必须大于 0".into());
    }
    let details = parse_feed_cursor(cursor)?;
    let mut outer = parse_query_pairs(cursor);
    let mut attach = parse_query_pairs(pair_value(&outer, "att").ok_or("分页游标缺少 att")?);
    let mut backend = parse_query_pairs(
        pair_value(&attach, "back_server_info").ok_or("分页游标缺少 back_server_info")?,
    );
    let load_count_in_outer = pair_value(&outer, "loadcount").is_some();
    set_pair_value(
        &mut backend,
        "offset",
        details.offset.saturating_add(offset_advance).to_string(),
    )?;
    set_pair_value(
        &mut attach,
        "back_server_info",
        serialize_query_pairs(&backend),
    )?;
    if !load_count_in_outer {
        set_or_append_pair_value(
            &mut attach,
            "loadcount",
            details.load_count.saturating_add(1).to_string(),
        );
    }
    set_pair_value(&mut outer, "att", serialize_query_pairs(&attach))?;
    if load_count_in_outer {
        set_pair_value(
            &mut outer,
            "loadcount",
            details.load_count.saturating_add(1).to_string(),
        )?;
    }
    Ok(serialize_query_pairs(&outer))
}

fn unresolved_skip_count(app: &tauri::AppHandle, owner_uin: &str) -> Result<u32, String> {
    let connection = open_database(app)?;
    connection
        .query_row(
            "SELECT COUNT(*) FROM archive_skips WHERE owner_uin=?1 AND resolved_at IS NULL",
            params![owner_uin],
            |row| row.get(0),
        )
        .map_err(|error| format!("读取异常跳过数量失败：{error}"))
}

fn known_skip_advance(
    app: &tauri::AppHandle,
    owner_uin: &str,
    details: FeedCursorDetails,
) -> Result<Option<(i64, String)>, String> {
    let connection = open_database(app)?;
    match connection.query_row(
        "SELECT offset_advance,error FROM archive_skips
         WHERE owner_uin=?1 AND cursor_offset=?2 AND base_time=?3 AND resolved_at IS NULL",
        params![owner_uin, details.offset, details.base_time],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(format!("读取已知异常位置失败：{error}")),
    }
}

struct SkipRecord<'a> {
    cursor: &'a str,
    resume_cursor: &'a str,
    page_number: u32,
    details: FeedCursorDetails,
    offset_advance: i64,
    error: &'a str,
}

fn record_archive_skip(
    app: &tauri::AppHandle,
    owner_uin: &str,
    record: SkipRecord<'_>,
) -> Result<(), String> {
    let connection = open_database(app)?;
    connection.execute(
        "INSERT INTO archive_skips
         (owner_uin,cursor,resume_cursor,page_number,cursor_offset,offset_advance,base_time,error,skipped_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(owner_uin,cursor_offset,base_time) DO UPDATE SET
          cursor=excluded.cursor,resume_cursor=excluded.resume_cursor,page_number=excluded.page_number,
          offset_advance=excluded.offset_advance,error=excluded.error,skipped_at=excluded.skipped_at,
          resolved_at=NULL,recovered_records=0",
        params![
            owner_uin,
            record.cursor,
            record.resume_cursor,
            record.page_number,
            record.details.offset,
            record.offset_advance,
            record.details.base_time,
            concise_archive_error(record.error),
            now(),
        ],
    ).map_err(|error| format!("保存异常跳过记录失败：{error}"))?;
    Ok(())
}

/// 限流暂停不应计入游标的陈旧计时：把续传位置的更新时间推到预计恢复时刻，
/// 这样倒计时结束后继续归档时能从原游标位置续传，而不是从头重扫。
fn freeze_checkpoint_during_pause(
    app: &tauri::AppHandle,
    owner_uin: &str,
    retry_at: i64,
) -> Result<(), String> {
    let connection = open_database(app)?;
    connection
        .execute(
            "UPDATE archive_checkpoints SET updated_at=?2 WHERE owner_uin=?1",
            params![owner_uin, retry_at],
        )
        .map_err(|error| format!("更新归档续传位置失败：{error}"))?;
    Ok(())
}

/// 收集查询结果并记录被丢弃的解析失败行，避免静默丢数据。
fn collect_rows_logged<T>(rows: impl Iterator<Item = Result<T, rusqlite::Error>>) -> Vec<T> {
    let mut items = Vec::new();
    let mut dropped = 0usize;
    for row in rows {
        match row {
            Ok(value) => items.push(value),
            Err(error) => {
                dropped += 1;
                eprintln!("读取归档数据行失败已跳过：{error}");
            }
        }
    }
    if dropped > 0 {
        eprintln!("本次查询共有 {dropped} 行解析失败被跳过");
    }
    items
}

fn checkpoint_is_stale(checkpoint: &ArchiveCheckpoint, current: i64) -> bool {
    current.saturating_sub(checkpoint.updated_at) >= ARCHIVE_CURSOR_MAX_AGE_SECONDS
}

pub(crate) fn reserve_archive_page(
    app: &tauri::AppHandle,
    owner_uin: &str,
) -> Result<Option<i64>, String> {
    let connection = open_database(app)?;
    let current = now();
    let state = connection.query_row(
        "SELECT window_started_at,requested_pages FROM archive_rate_limits WHERE owner_uin=?1",
        params![owner_uin],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    match state {
        Ok((started_at, pages))
            if current - started_at < ARCHIVE_RATE_WINDOW_SECONDS
                && pages >= ARCHIVE_RATE_PAGE_LIMIT =>
        {
            Ok(Some(started_at + ARCHIVE_RATE_WINDOW_SECONDS))
        }
        Ok((started_at, _)) if current - started_at >= ARCHIVE_RATE_WINDOW_SECONDS => {
            connection.execute(
                "UPDATE archive_rate_limits SET window_started_at=?2,requested_pages=1 WHERE owner_uin=?1",
                params![owner_uin, current],
            ).map_err(|error| format!("重置归档频率窗口失败：{error}"))?;
            Ok(None)
        }
        Ok(_) => {
            connection.execute(
                "UPDATE archive_rate_limits SET requested_pages=requested_pages+1 WHERE owner_uin=?1",
                params![owner_uin],
            ).map_err(|error| format!("记录归档请求频率失败：{error}"))?;
            Ok(None)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            connection.execute(
                "INSERT INTO archive_rate_limits(owner_uin,window_started_at,requested_pages) VALUES (?1,?2,1)",
                params![owner_uin, current],
            ).map_err(|error| format!("创建归档频率窗口失败：{error}"))?;
            Ok(None)
        }
        Err(error) => Err(format!("读取归档请求频率失败：{error}")),
    }
}

fn load_checkpoint(
    app: &tauri::AppHandle,
    owner_uin: &str,
) -> Result<Option<ArchiveCheckpoint>, String> {
    let connection = open_database(app)?;
    match connection.query_row(
        "SELECT attach_info,pages,fetched,saved,updated_at FROM archive_checkpoints WHERE owner_uin=?1",
        params![owner_uin],
        |row| {
            Ok(ArchiveCheckpoint {
                cursor: row.get(0)?,
                pages: row.get(1)?,
                fetched: row.get(2)?,
                saved: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    ) {
        Ok(checkpoint) if !checkpoint.cursor.trim().is_empty() => Ok(Some(checkpoint)),
        Ok(_) => Ok(None),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(format!("读取归档续传位置失败：{error}")),
    }
}

fn save_original_dynamic(
    transaction: &rusqlite::Transaction<'_>,
    owner_uin: &str,
    feed: &Value,
) -> Result<(), String> {
    let Some(original) = feed.get("original") else {
        return Ok(());
    };
    let Some(cell_id) = text_at(original, "/cell_id/cellid") else {
        return Ok(());
    };
    let original_appid = original
        .pointer("/cell_comm/appid")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let original_key = text_at(original, "/cell_comm/feedskey").unwrap_or_default();
    let is_guestbook = original_appid == 334 || original_key.starts_with("334_");
    let published_at = original
        .pointer("/cell_comm/time")
        .and_then(Value::as_i64)
        .or_else(|| feed.pointer("/comm/time").and_then(Value::as_i64))
        .unwrap_or(0);
    let content = if is_guestbook {
        text_at(feed, "/summary/summary")
    } else {
        text_at(original, "/cell_summary/summary")
    };
    let author_uin = if is_guestbook {
        text_at(feed, "/userinfo/user/uin")
    } else {
        text_at(original, "/cell_userinfo/user/uin")
    };
    let author_name = if is_guestbook {
        text_at(feed, "/userinfo/user/nickname")
    } else {
        text_at(original, "/cell_userinfo/user/nickname")
    };
    let category = if is_guestbook {
        "guestbook"
    } else if author_uin.as_deref() == Some(owner_uin) {
        "self"
    } else {
        "other"
    };
    let pictures_json = original
        .get("cell_pic")
        .filter(|value| !value.is_null())
        .map(Value::to_string);
    let video_json = original
        .get("cell_video")
        .filter(|value| !value.is_null())
        .map(Value::to_string);
    transaction.execute(
        "INSERT INTO archive_dynamics
         (owner_uin,cell_id,published_at,content,author_uin,author_name,category,pictures_json,video_json,raw_original_json,archived_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(owner_uin,cell_id) DO UPDATE SET
          published_at=excluded.published_at,content=excluded.content,author_uin=excluded.author_uin,
          author_name=excluded.author_name,category=excluded.category,pictures_json=COALESCE(excluded.pictures_json,archive_dynamics.pictures_json),
          video_json=COALESCE(excluded.video_json,archive_dynamics.video_json),
          raw_original_json=excluded.raw_original_json,archived_at=excluded.archived_at",
        params![owner_uin,cell_id,published_at,content,author_uin,author_name,category,pictures_json,video_json,original.to_string(),now()],
    ).map_err(|error| format!("保存原动态失败：{error}"))?;
    Ok(())
}

fn picture_url_candidates(json: Option<String>) -> Vec<Vec<String>> {
    let Some(value) = json.and_then(|text| serde_json::from_str::<Value>(&text).ok()) else {
        return vec![];
    };
    value
        .pointer("/picdata/pic")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pic| {
            let photo_urls = pic.get("photourl")?;
            let values = match photo_urls {
                Value::Array(items) => items.iter().collect::<Vec<_>>(),
                Value::Object(items) => items.values().collect::<Vec<_>>(),
                _ => vec![],
            };
            let candidates = values
                .into_iter()
                .filter_map(|item| {
                    let url = item.get("url")?.as_str()?.trim();
                    if url.is_empty() {
                        return None;
                    }
                    Some(url.to_owned())
                })
                .collect::<Vec<_>>();
            let mut candidates = candidates;
            if let Some(url) = pic
                .pointer("/busi_param/-1")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|url| !url.is_empty())
            {
                candidates.push(url.to_owned());
            }
            let mut seen = HashSet::new();
            let urls = candidates
                .into_iter()
                .map(|url| {
                    if url.starts_with("//") {
                        format!("https:{url}")
                    } else {
                        url
                    }
                })
                .filter(|url| seen.insert(url.clone()))
                .collect::<Vec<_>>();
            (!urls.is_empty()).then_some(urls)
        })
        .collect()
}

pub(crate) fn picture_urls(json: Option<String>) -> Vec<String> {
    picture_url_candidates(json)
        .into_iter()
        .filter_map(|urls| urls.into_iter().next())
        .collect()
}

fn archived_image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.get(4..12).is_some_and(|value| {
        value.starts_with(b"ftyp") && (&value[4..8] == b"avif" || &value[4..8] == b"avis")
    }) {
        Some("avif")
    } else {
        None
    }
}

fn is_qq_missing_image_placeholder(bytes: &[u8]) -> bool {
    bytes.get(6..10).is_some_and(|size| {
        let width = u16::from_le_bytes([size[0], size[1]]);
        let height = u16::from_le_bytes([size[2], size[3]]);
        (bytes.len() == 2_038 && bytes.starts_with(b"GIF89a") && width == 340 && height == 320)
            || (bytes.len() == 2_687
                && bytes.starts_with(b"GIF89a")
                && width == 340
                && height == 320)
            || (bytes.len() == 1_643 && bytes.starts_with(b"GIF87a") && width == 99 && height == 99)
            || (bytes.len() == 1_547 && bytes.starts_with(b"GIF87a") && width == 98 && height == 98)
    })
}

pub(crate) fn existing_archived_image(
    image_dir: &std::path::Path,
    file_stem: &str,
) -> Option<PathBuf> {
    ["jpg", "png", "gif", "webp", "avif", "bmp"]
        .into_iter()
        .map(|extension| image_dir.join(format!("{file_stem}.{extension}")))
        .find_map(|path| {
            if !path.metadata().is_ok_and(|metadata| metadata.len() > 32) {
                return None;
            }
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"))
                && fs::read(&path).is_ok_and(|bytes| is_qq_missing_image_placeholder(&bytes))
            {
                let _ = fs::remove_file(&path);
                return None;
            }
            Some(path)
        })
}

/// 归档图片目录：按账号隔离，动态图片与相册照片都存放在这里。
pub(crate) fn archived_image_dir(app: &tauri::AppHandle, uin: &str) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取图片归档目录：{error}"))?
        .join("images")
        .join(uin);
    fs::create_dir_all(&dir).map_err(|error| format!("无法创建图片归档目录：{error}"))?;
    Ok(dir)
}

/// 视频缓存文件路径（按账号隔离）。
pub(crate) fn video_cache_path(
    app: &tauri::AppHandle,
    uin: &str,
    dynamic_id: i64,
) -> Result<PathBuf, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("无法获取视频缓存目录：{error}"))?
        .join("videos");
    fs::create_dir_all(&cache_dir).map_err(|error| format!("无法创建视频缓存目录：{error}"))?;
    Ok(cache_dir.join(format!("{uin}-{dynamic_id}.mp4")))
}

pub(crate) struct DownloadedImage {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
}

/// 按“候选地址 + Cookie/Referer 组合矩阵”下载 QQ 图片，仅白名单域名会携带登录态，
/// 并校验内容确为图片（拒绝占位图）。返回第一个成功的结果。
pub(crate) async fn download_qzone_image(
    auth: &crate::qlogin::QzoneAuth,
    candidates: &[String],
    client: &reqwest::Client,
) -> Result<DownloadedImage, String> {
    let mut last_error = String::new();
    for url in candidates {
        if !is_trusted_media_host(url) {
            last_error = "跳过非 QQ 域名的图片地址".into();
            continue;
        }
        for (with_cookie, with_referer) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let mut request = client
                .get(url)
                .header(reqwest::header::USER_AGENT, &auth.user_agent)
                .header(
                    reqwest::header::ACCEPT,
                    "image/avif,image/webp,image/png,image/jpeg,image/*,*/*;q=0.8",
                )
                .header(
                    reqwest::header::ACCEPT_LANGUAGE,
                    "zh-CN,zh;q=0.9,en;q=0.8,en-GB;q=0.7,en-US;q=0.6,zh-TW;q=0.5",
                );
            if with_cookie {
                request = request.header(reqwest::header::COOKIE, &auth.cookie_header);
            }
            if with_referer {
                request = request.header(reqwest::header::REFERER, "https://user.qzone.qq.com/");
            }
            match request.send().await {
                Ok(response) if response.status().is_success() => {
                    if response
                        .content_length()
                        .is_some_and(|length| length > 50 * 1024 * 1024)
                    {
                        last_error = "图片超过 50 MB 安全限制".into();
                        continue;
                    }
                    match response.bytes().await {
                        Ok(bytes) => {
                            let Some(extension) = archived_image_extension(&bytes) else {
                                last_error = "QQ 返回了非图片内容".into();
                                continue;
                            };
                            if is_qq_missing_image_placeholder(&bytes) {
                                last_error = "QQ 返回了图片不存在占位图".into();
                                continue;
                            }
                            return Ok(DownloadedImage {
                                bytes: bytes.to_vec(),
                                extension,
                            });
                        }
                        Err(error) => last_error = format!("读取图片数据失败：{error}"),
                    }
                }
                Ok(response) => last_error = format!("HTTP {}", response.status()),
                Err(error) => last_error = format!("请求图片失败：{error}"),
            }
        }
    }
    Err(format!("所有 QQ 图片地址均加载失败：{last_error}"))
}

pub(crate) fn set_file_timestamp(path: &std::path::Path, timestamp_secs: i64) {
    if timestamp_secs <= 0 {
        return;
    }
    let ft = filetime::FileTime::from_unix_time(timestamp_secs, 0);
    let _ = filetime::set_file_times(path, ft, ft);
}

/// 原子落盘：先写临时文件再改名，避免半张图片占用正式文件名导致续传误判。
#[allow(dead_code)]
pub(crate) fn persist_downloaded_image(
    image_dir: &std::path::Path,
    file_stem: &str,
    downloaded: &DownloadedImage,
) -> Result<PathBuf, String> {
    persist_downloaded_image_with_time(image_dir, file_stem, downloaded, None)
}

pub(crate) fn persist_downloaded_image_with_time(
    image_dir: &std::path::Path,
    file_stem: &str,
    downloaded: &DownloadedImage,
    timestamp_secs: Option<i64>,
) -> Result<PathBuf, String> {
    let path = image_dir.join(format!("{file_stem}.{}", downloaded.extension));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = image_dir.join(format!("{file_stem}-{nonce}.part"));
    fs::write(&temporary, &downloaded.bytes)
        .map_err(|error| format!("写入图片归档失败：{error}"))?;
    if let Err(error) = fs::rename(&temporary, &path) {
        if !path.exists() {
            let _ = fs::remove_file(&temporary);
            return Err(format!("保存图片归档失败：{error}"));
        }
        let _ = fs::remove_file(&temporary);
    }
    if let Some(ts) = timestamp_secs {
        set_file_timestamp(&path, ts);
    }
    Ok(path)
}

#[tauri::command]
pub async fn load_archived_image(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    state: tauri::State<'_, ArchiveState>,
    id: i64,
    picture_index: usize,
) -> Result<String, String> {
    let auth = login.qzone_auth().await?;
    let image_dir = archived_image_dir(&app, &auth.uin)?;
    let file_stem = format!("{id}-{picture_index}");
    if let Some(path) = existing_archived_image(&image_dir, &file_stem) {
        return Ok(path.to_string_lossy().into_owned());
    }

    let (pictures_json, published_at) = {
        let connection = open_database(&app)?;
        connection
            .query_row(
                "SELECT pictures_json, published_at FROM archive_dynamics WHERE id=?1 AND owner_uin=?2",
                params![id, auth.uin],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => "当前账号中不存在这条图片归档".into(),
                _ => format!("读取图片归档失败：{error}"),
            })?
    };
    let candidates = picture_url_candidates(pictures_json)
        .into_iter()
        .nth(picture_index)
        .ok_or("该图片没有保存可用的 QQ 地址")?;
    let _permit = state
        .image_downloads
        .acquire()
        .await
        .map_err(|_| "图片下载队列已关闭")?;
    if let Some(path) = existing_archived_image(&image_dir, &file_stem) {
        return Ok(path.to_string_lossy().into_owned());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|error| format!("创建图片请求客户端失败：{error}"))?;
    let downloaded = download_qzone_image(&auth, &candidates, &client).await?;
    let path = persist_downloaded_image_with_time(
        &image_dir,
        &file_stem,
        &downloaded,
        Some(published_at),
    )?;
    Ok(path.to_string_lossy().into_owned())
}

pub(crate) fn video_urls(json: Option<String>) -> Vec<String> {
    let Some(value) = json.and_then(|text| serde_json::from_str::<Value>(&text).ok()) else {
        return vec![];
    };
    let mut urls = Vec::new();
    if let Some(url) = value.get("videourl").and_then(Value::as_str) {
        urls.push(url.to_owned());
    }
    if let Some(items) = value.get("videourls").and_then(Value::as_object) {
        for url in items
            .values()
            .filter_map(|item| item.get("url").and_then(Value::as_str))
        {
            if !urls.iter().any(|saved| saved == url) {
                urls.push(url.to_owned());
            }
        }
    }
    urls
}

pub(crate) fn video_cover_url(json: Option<String>) -> Option<String> {
    let value = json.and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    value
        .pointer("/coverurl/0/url")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("coverurl")?
                .as_object()?
                .values()
                .find_map(|item| item.get("url")?.as_str())
        })
        .map(str::to_owned)
}

#[tauri::command]
pub async fn load_archived_video(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    id: i64,
) -> Result<String, String> {
    let auth = login.qzone_auth().await?;
    let cache_path = video_cache_path(&app, &auth.uin, id)?;
    if cache_path
        .metadata()
        .is_ok_and(|metadata| metadata.len() > 1024)
    {
        return Ok(cache_path.to_string_lossy().into_owned());
    }
    let video_json = {
        let connection = open_database(&app)?;
        connection
            .query_row(
                "SELECT video_json FROM archive_dynamics WHERE id=?1 AND owner_uin=?2",
                params![id, auth.uin],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => "当前账号中不存在这条视频归档".into(),
                _ => format!("读取视频归档失败：{error}"),
            })?
    };
    let candidates = video_urls(video_json);
    if candidates.is_empty() {
        return Err("该归档没有可用的视频地址".into());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|error| format!("创建视频请求客户端失败：{error}"))?;
    let mut last_error = String::new();
    let mut rejected = false;
    for url in candidates {
        if !is_trusted_media_host(&url) {
            last_error = "跳过非 QQ 域名的视频地址".into();
            continue;
        }
        for (with_cookie, with_referer) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let mut request = client
                .get(&url)
                .header(reqwest::header::USER_AGENT, &auth.user_agent)
                .header(
                    reqwest::header::ACCEPT,
                    "video/mp4,video/*;q=0.9,application/octet-stream;q=0.8,*/*;q=0.5",
                )
                .header(
                    reqwest::header::ACCEPT_LANGUAGE,
                    "zh-CN,zh;q=0.9,en;q=0.8,en-GB;q=0.7,en-US;q=0.6,zh-TW;q=0.5",
                );
            if with_cookie {
                request = request.header(reqwest::header::COOKIE, &auth.cookie_header);
            }
            if with_referer {
                request = request.header(reqwest::header::REFERER, "https://user.qzone.qq.com/");
            }
            match request.send().await {
                Ok(mut response) if response.status().is_success() => {
                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if response
                        .content_length()
                        .is_some_and(|length| length > 512 * 1024 * 1024)
                    {
                        last_error = "视频超过 512 MB 安全限制".into();
                        continue;
                    }
                    let nonce = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos();
                    let temporary = cache_path.with_extension(format!("part-{nonce}"));
                    match stream_video_to_cache(&mut response, &content_type, &temporary).await {
                        Ok(true) => {
                            if fs::rename(&temporary, &cache_path).is_err() && !cache_path.exists()
                            {
                                let _ = fs::remove_file(&temporary);
                                return Err("保存视频缓存失败：无法落盘临时文件".into());
                            }
                            let _ = fs::remove_file(&temporary);
                            return Ok(cache_path.to_string_lossy().into_owned());
                        }
                        Ok(false) => {
                            let _ = fs::remove_file(&temporary);
                            last_error = format!(
                                "QQ 返回了非视频内容（{}）",
                                if content_type.is_empty() {
                                    "未知类型"
                                } else {
                                    &content_type
                                }
                            );
                        }
                        Err(error) => {
                            let _ = fs::remove_file(&temporary);
                            last_error = format!("读取视频数据失败：{error}");
                        }
                    }
                }
                Ok(response) => {
                    rejected |= response.status() == reqwest::StatusCode::FORBIDDEN;
                    last_error = format!("HTTP {}", response.status());
                }
                Err(error) => last_error = format!("请求视频失败：{error}"),
            }
        }
    }
    if rejected {
        Err("QQ 拒绝了视频请求（HTTP 403），该归档的视频临时签名可能已经过期，请重新归档以更新视频地址".into())
    } else {
        Err(format!("所有视频地址均加载失败：{last_error}"))
    }
}

fn set_progress(state: &ArchiveState, update: impl FnOnce(&mut ArchiveProgress)) {
    if let Ok(mut progress) = state.progress.lock() {
        update(&mut progress);
    }
}

fn concise_archive_error(error: &str) -> String {
    let normalized = error.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let summary = chars.by_ref().take(240).collect::<String>();
    if chars.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}

/// 媒体下载前的白名单校验：仅允许 https 访问 QQ 系图片/视频域名，
/// 拒绝任意来源地址携带会话 Cookie 发起请求。
fn is_trusted_media_host(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    ["qq.com", "qpic.cn", "gtimg.cn", "gtimg.com", "qlogo.cn"]
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// 流式下载视频到临时文件：先校验 MP4 魔数与内容类型，再边下边写盘，
/// 避免数百 MB 的视频整体读入内存。Ok(true)=完整写入；Ok(false)=非视频内容。
async fn stream_video_to_cache(
    response: &mut reqwest::Response,
    content_type: &str,
    temporary: &std::path::Path,
) -> std::io::Result<bool> {
    use std::io::Write;
    let looks_like_video = |head: &[u8]| {
        content_type.starts_with("video/")
            || content_type.contains("octet-stream")
            || head
                .get(4..12)
                .is_some_and(|value| value.windows(4).any(|part| part == b"ftyp"))
    };
    let mut file: Option<std::fs::File> = None;
    let mut buffered: Vec<u8> = Vec::with_capacity(128 * 1024);
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if file.is_none() {
                    buffered.extend_from_slice(&chunk);
                    if buffered.len() < 16 {
                        continue;
                    }
                    if !looks_like_video(&buffered) {
                        return Ok(false);
                    }
                    let mut created = std::fs::File::create(temporary)?;
                    created.write_all(&buffered)?;
                    file = Some(created);
                } else if let Some(file) = file.as_mut() {
                    file.write_all(&chunk)?;
                }
            }
            Ok(None) => {
                let Some(mut file) = file.take() else {
                    return Ok(false);
                };
                file.flush()?;
                return Ok(true);
            }
            Err(error) => return Err(std::io::Error::other(format!("{error}"))),
        }
    }
}

async fn fetch_after_skipped_cursor(
    app: &tauri::AppHandle,
    login: &QLoginState,
    archive: &ArchiveState,
    owner_uin: &str,
    cursor: &str,
    first_advance: i64,
    interval_ms: u64,
) -> Result<(qzone::FeedPage, String, i64), String> {
    let first_advance = first_advance.clamp(1, ARCHIVE_SKIP_MAX_OFFSET_ADVANCE);
    let mut last_error = None;
    let mut last_failed_advance = first_advance.saturating_sub(1);
    let mut best: Option<(qzone::FeedPage, String, i64)> = None;
    // 整个异常恢复过程按一次"页"计费：探测请求自带间隔节流，
    // 避免一次恢复就重复消耗几十个限流配额、把正常归档打停。
    if let Some(retry_at) = reserve_archive_page(app, owner_uin)? {
        return Err(format!("ARCHIVE_RATE_LIMIT:{retry_at}"));
    }
    for offset_advance in skip_probe_offsets(first_advance) {
        set_progress(archive, |progress| {
            progress.message =
                format!("已记录异常位置，正在尝试从偏移 +{offset_advance} 恢复归档…");
        });
        let candidate = advance_feed_cursor(cursor, offset_advance)?;
        match qzone::fetch_feeds_once(login, "2", Some(&candidate)).await {
            Ok(page) => {
                best = Some((page, candidate, offset_advance));
                break;
            }
            Err(error) if qzone::feed_error_can_skip(&error) => {
                last_failed_advance = offset_advance;
                last_error = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(archive_page_delay_ms(
                    interval_ms,
                )))
                .await;
            }
            Err(error) => return Err(error),
        }
    }
    let Some(mut best) = best else {
        return Err(format!(
            "异常位置已保存到待重试列表，但向后探测至偏移 +{} 后仍无法取得下一页：{}",
            ARCHIVE_SKIP_MAX_OFFSET_ADVANCE,
            concise_archive_error(last_error.as_deref().unwrap_or("未知接口错误"))
        ));
    };

    let mut low = last_failed_advance.saturating_add(1);
    let mut high = best.2.saturating_sub(1);
    while low <= high {
        let offset_advance = low + (high - low) / 2;
        set_progress(archive, |progress| {
            progress.message =
                format!("已找到可恢复位置，正在缩小跳过范围（偏移 +{offset_advance}）…");
        });
        let candidate = advance_feed_cursor(cursor, offset_advance)?;
        match qzone::fetch_feeds_once(login, "2", Some(&candidate)).await {
            Ok(page) => {
                best = (page, candidate, offset_advance);
                high = offset_advance.saturating_sub(1);
            }
            Err(error) if qzone::feed_error_can_skip(&error) => {
                low = offset_advance.saturating_add(1);
            }
            Err(error) => return Err(error),
        }
        tokio::time::sleep(std::time::Duration::from_millis(archive_page_delay_ms(
            interval_ms,
        )))
        .await;
    }
    Ok(best)
}

fn skip_probe_offsets(first_advance: i64) -> Vec<i64> {
    let first_advance = first_advance.clamp(1, ARCHIVE_SKIP_MAX_OFFSET_ADVANCE);
    let mut offsets = vec![first_advance];
    let mut candidate = 1_i64;
    while candidate <= first_advance && candidate < ARCHIVE_SKIP_MAX_OFFSET_ADVANCE {
        candidate = candidate.saturating_mul(2);
    }
    while candidate < ARCHIVE_SKIP_MAX_OFFSET_ADVANCE {
        offsets.push(candidate);
        candidate = candidate.saturating_mul(2);
    }
    if offsets.last().copied() != Some(ARCHIVE_SKIP_MAX_OFFSET_ADVANCE) {
        offsets.push(ARCHIVE_SKIP_MAX_OFFSET_ADVANCE);
    }
    offsets
}

#[tauri::command]
pub async fn start_feed_archive(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    archive: tauri::State<'_, ArchiveState>,
    interval_ms: u64,
) -> Result<ArchiveProgress, String> {
    let interval_ms = interval_ms.clamp(2_000, 30_000);
    if archive.batch_retrying.load(Ordering::Relaxed) {
        return Err("正在批量重试异常位置，请等待完成或停止后再开始归档".into());
    }
    {
        let mut progress = archive.progress.lock().map_err(|_| "归档状态锁已损坏")?;
        if progress.status == "running" {
            return Err("已有归档任务正在运行".into());
        }
        *progress = ArchiveProgress {
            status: "running",
            pages: 0,
            fetched: 0,
            saved: 0,
            skipped: 0,
            message: "正在准备归档…".into(),
            retry_at: None,
            batch_retry: None,
        };
    }
    archive.cancel.store(false, Ordering::Relaxed);
    let owner_uin = login.qzone_auth().await?.uin;
    let saved_skip_count = unresolved_skip_count(&app, &owner_uin)?;
    set_progress(&archive, |progress| progress.skipped = saved_skip_count);
    let checkpoint = load_checkpoint(&app, &owner_uin)?;
    let stale_checkpoint = checkpoint
        .as_ref()
        .is_some_and(|value| checkpoint_is_stale(value, now()));
    let mut reset_checkpoint_stats = stale_checkpoint;
    let mut cursor = checkpoint
        .as_ref()
        .filter(|_| !stale_checkpoint)
        .map(|value| value.cursor.clone());
    let mut seen_cursors = HashSet::new();
    if stale_checkpoint {
        set_progress(&archive, |progress| {
            progress.message =
                "上次分页位置已超过 10 分钟，正在从第一页重新校验；已保存记录会自动去重。".into();
        });
    } else if let Some(checkpoint) = checkpoint.as_ref() {
        let saved_cursor = &checkpoint.cursor;
        seen_cursors.insert(saved_cursor.clone());
        set_progress(&archive, |progress| {
            progress.pages = checkpoint.pages;
            progress.fetched = checkpoint.fetched;
            progress.saved = checkpoint.saved;
            progress.message = format!("已恢复上次进度：{} 页，正在继续归档…", checkpoint.pages);
        });
    }
    let result: Result<(), String> = async {
        loop {
            if archive.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Some(retry_at) = reserve_archive_page(&app, &owner_uin)? {
                return Err(format!("ARCHIVE_RATE_LIMIT:{retry_at}"));
            }
            let mut skipped_page: Option<(String, String, FeedCursorDetails, i64, String)> = None;
            let page = if let Some(current_cursor) = cursor.as_deref() {
                let known_skip = match parse_feed_cursor(current_cursor) {
                    Ok(details) => {
                        known_skip_advance(&app, &owner_uin, details)?.map(|known| (details, known))
                    }
                    Err(_) => None,
                };
                if let Some((details, (known_advance, known_error))) = known_skip {
                    let (page, resume_cursor, offset_advance) = fetch_after_skipped_cursor(
                        &app,
                        &login,
                        &archive,
                        &owner_uin,
                        current_cursor,
                        known_advance,
                        interval_ms,
                    )
                    .await?;
                    skipped_page = Some((
                        current_cursor.to_owned(),
                        resume_cursor,
                        details,
                        offset_advance,
                        known_error,
                    ));
                    page
                } else {
                    match qzone::fetch_feeds_persistent(&login, "2", Some(current_cursor)).await {
                        Ok(page) => page,
                        Err(error) if qzone::feed_error_can_skip(&error) => {
                            let details =
                                parse_feed_cursor(current_cursor).map_err(|cursor_error| {
                                    format!("{error}；且无法自动跳过该页：{cursor_error}")
                                })?;
                            let page_number = archive
                                .progress
                                .lock()
                                .map_err(|_| "归档状态锁已损坏")?
                                .pages
                                .saturating_add(1);
                            record_archive_skip(
                                &app,
                                &owner_uin,
                                SkipRecord {
                                    cursor: current_cursor,
                                    resume_cursor: current_cursor,
                                    page_number,
                                    details,
                                    offset_advance: 0,
                                    error: &error,
                                },
                            )?;
                            let skip_count = unresolved_skip_count(&app, &owner_uin)?;
                            set_progress(&archive, |progress| {
                                progress.skipped = skip_count;
                                progress.message = format!(
                                    "第 {page_number} 页发生异常，已加入待重试列表，正在寻找后续可恢复位置…"
                                );
                            });
                            let (page, resume_cursor, offset_advance) = fetch_after_skipped_cursor(
                                &app,
                                &login,
                                &archive,
                                &owner_uin,
                                current_cursor,
                                1,
                                interval_ms,
                            )
                            .await?;
                            skipped_page = Some((
                                current_cursor.to_owned(),
                                resume_cursor,
                                details,
                                offset_advance,
                                error,
                            ));
                            page
                        }
                        Err(error) => return Err(error),
                    }
                }
            } else {
                qzone::fetch_feeds_persistent(&login, "1", None).await?
            };
            let fetched = page.feeds.len() as u64;
            let next = if page.has_more {
                Some(
                    page.attach_info
                        .as_deref()
                        .ok_or("接口表示还有数据，但未返回分页游标")?,
                )
            } else {
                None
            };
            if let Some(next_cursor) = next {
                if !seen_cursors.insert(next_cursor.to_owned()) {
                    return Err("检测到重复分页游标，已停止以避免死循环".into());
                }
            }
            let did_skip = skipped_page.is_some();
            if let Some((failed_cursor, resume_cursor, details, offset_advance, error)) =
                skipped_page.as_ref()
            {
                let page_number = archive
                    .progress
                    .lock()
                    .map_err(|_| "归档状态锁已损坏")?
                    .pages
                    .saturating_add(1);
                record_archive_skip(
                    &app,
                    &owner_uin,
                    SkipRecord {
                        cursor: failed_cursor,
                        resume_cursor,
                        page_number,
                        details: *details,
                        offset_advance: *offset_advance,
                        error,
                    },
                )?;
            }
            let saved = save_page(&app, &owner_uin, &page.feeds, next, reset_checkpoint_stats)?;
            reset_checkpoint_stats = false;
            let skip_count = unresolved_skip_count(&app, &owner_uin)?;
            set_progress(&archive, |progress| {
                progress.pages += 1;
                progress.fetched += fetched;
                progress.saved += saved;
                progress.skipped = skip_count;
                progress.message = if did_skip {
                    format!(
                        "已跳过 1 个异常位置并继续归档；当前 {} 页，共 {} 条记录",
                        progress.pages, progress.fetched
                    )
                } else {
                    format!(
                        "已归档 {} 页，共 {} 条记录",
                        progress.pages, progress.fetched
                    )
                };
            });
            if !page.has_more {
                return Ok(());
            }
            cursor = next.map(str::to_owned);
            tokio::time::sleep(std::time::Duration::from_millis(archive_page_delay_ms(
                interval_ms,
            )))
            .await;
        }
    }
    .await;
    match &result {
        Ok(()) if archive.cancel.load(Ordering::Relaxed) => set_progress(&archive, |p| {
            p.status = "cancelled";
            p.message = "归档已取消".into();
            p.retry_at = None;
        }),
        Ok(()) => set_progress(&archive, |p| {
            p.status = "completed";
            p.message = if p.skipped > 0 {
                format!(
                    "归档完成，共保存 {} 条记录；另有 {} 个异常位置已跳过，可在下方单独重试",
                    p.saved, p.skipped
                )
            } else {
                format!("归档完成，共保存 {} 条记录", p.saved)
            };
            p.retry_at = None;
        }),
        Err(error) if error.starts_with("ARCHIVE_RATE_LIMIT:") => {
            let retry_at = error
                .trim_start_matches("ARCHIVE_RATE_LIMIT:")
                .parse::<i64>()
                .ok();
            if let Some(retry_at) = retry_at {
                if let Err(freeze_error) =
                    freeze_checkpoint_during_pause(&app, &owner_uin, retry_at)
                {
                    eprintln!("限流期间冻结续传位置失败：{freeze_error}");
                }
            }
            set_progress(&archive, |p| {
                p.status = "limited";
                p.retry_at = retry_at;
                p.message = "为防止接口请求过于频繁，每 10 分钟最多归档 300 页。达到限制后已安全暂停，倒计时结束即可从当前进度继续归档。".into();
            });
        }
        Err(_error) => set_progress(&archive, |p| {
            let detail = serde_json::json!({
                "event": "qzone_archive_task_error",
                "error": _error,
                "pages": p.pages,
                "fetched": p.fetched,
                "saved": p.saved,
                "ownerUin": owner_uin,
            });
            eprintln!(
                "\n================ QZONE ARCHIVE TASK ERROR ================\n{}\n================ END QZONE ARCHIVE TASK ERROR ================\n",
                serde_json::to_string_pretty(&detail).unwrap_or_else(|_| detail.to_string())
            );
            p.status = "error";
            p.message = format!("归档失败：{}", concise_archive_error(_error));
            p.retry_at = None;
        }),
    }
    let progress = archive
        .progress
        .lock()
        .map_err(|_| "归档状态锁已损坏")?
        .clone();
    if result
        .as_ref()
        .is_err_and(|error| error.starts_with("ARCHIVE_RATE_LIMIT:"))
    {
        Ok(progress)
    } else {
        result.map(|_| progress)
    }
}

#[tauri::command]
pub fn get_archive_progress(
    state: tauri::State<'_, ArchiveState>,
) -> Result<ArchiveProgress, String> {
    state
        .progress
        .lock()
        .map(|value| value.clone())
        .map_err(|_| "归档状态锁已损坏".into())
}

#[tauri::command]
pub async fn list_archive_skips(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<Vec<ArchiveSkipItem>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT id,page_number,cursor_offset,offset_advance,base_time,error,skipped_at,
                    retry_count,last_retry_at,resolved_at,recovered_records
             FROM archive_skips WHERE owner_uin=?1
             ORDER BY resolved_at IS NOT NULL, skipped_at DESC",
        )
        .map_err(|error| format!("读取异常跳过列表失败：{error}"))?;
    let rows = statement
        .query_map(params![owner_uin], |row| {
            Ok(ArchiveSkipItem {
                id: row.get(0)?,
                page_number: row.get(1)?,
                cursor_offset: row.get(2)?,
                offset_advance: row.get(3)?,
                base_time: row.get(4)?,
                error: row.get(5)?,
                skipped_at: row.get(6)?,
                retry_count: row.get(7)?,
                last_retry_at: row.get(8)?,
                resolved_at: row.get(9)?,
                recovered_records: row.get(10)?,
            })
        })
        .map_err(|error| format!("查询异常跳过列表失败：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("解析异常跳过列表失败：{error}"))
}

#[tauri::command]
pub async fn clear_resolved_archive_skips(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<u64, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let removed = connection
        .execute(
            "DELETE FROM archive_skips WHERE owner_uin=?1 AND resolved_at IS NOT NULL",
            params![owner_uin],
        )
        .map_err(|error| format!("清理已恢复的异常跳过记录失败：{error}"))?;
    Ok(removed as u64)
}

#[tauri::command]
pub async fn retry_archive_skip(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    archive: tauri::State<'_, ArchiveState>,
    id: i64,
) -> Result<ArchiveSkipRetryResult, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    retry_single_skip(&app, &login, &archive, &owner_uin, id).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSkipBatchRetryResult {
    total: u32,
    recovered: u32,
    failed: u32,
    recovered_records: u64,
}

#[tauri::command]
pub async fn retry_all_archive_skips(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    archive: tauri::State<'_, ArchiveState>,
    interval_ms: u64,
) -> Result<ArchiveSkipBatchRetryResult, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    ensure_archive_idle(&archive)?;
    if archive
        .batch_retrying
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return Err("已有批量重试在进行中".into());
    }
    let pending_ids = {
        let connection = open_database(&app)?;
        let mut statement = connection
            .prepare(
                "SELECT id FROM archive_skips WHERE owner_uin=?1 AND resolved_at IS NULL
                 ORDER BY skipped_at ASC",
            )
            .map_err(|error| format!("读取异常跳过列表失败：{error}"))?;
        let rows = statement
            .query_map(params![owner_uin], |row| row.get(0))
            .map_err(|error| format!("查询异常跳过列表失败：{error}"))?;
        rows.collect::<Result<Vec<i64>, _>>()
            .map_err(|error| format!("解析异常跳过列表失败：{error}"))?
    };
    let total = pending_ids.len() as u32;
    let mut result = ArchiveSkipBatchRetryResult {
        total,
        recovered: 0,
        failed: 0,
        recovered_records: 0,
    };
    archive.batch_cancel.store(false, Ordering::Relaxed);
    for (index, id) in pending_ids.into_iter().enumerate() {
        if archive.batch_cancel.load(Ordering::Relaxed) {
            break;
        }
        set_progress(&archive, |progress| {
            progress.batch_retry = Some(BatchRetryProgress {
                current: index as u32 + 1,
                total,
                recovered: result.recovered,
                failed: result.failed,
                recovered_records: result.recovered_records,
            });
        });
        match retry_single_skip(&app, &login, &archive, &owner_uin, id).await {
            Ok(outcome) => {
                if outcome.success {
                    result.recovered += 1;
                    result.recovered_records += outcome.recovered_records;
                } else {
                    result.failed += 1;
                }
                set_progress(&archive, |progress| {
                    progress.batch_retry = Some(BatchRetryProgress {
                        current: index as u32 + 1,
                        total,
                        recovered: result.recovered,
                        failed: result.failed,
                        recovered_records: result.recovered_records,
                    });
                });
            }
            Err(error) => {
                if error.starts_with("请求频率保护中") || error.starts_with("归档任务运行中")
                {
                    break;
                }
                result.failed += 1;
                set_progress(&archive, |progress| {
                    progress.batch_retry = Some(BatchRetryProgress {
                        current: index as u32 + 1,
                        total,
                        recovered: result.recovered,
                        failed: result.failed,
                        recovered_records: result.recovered_records,
                    });
                });
            }
        }
        // 与归档任务保持一致的节奏，避免批量重试触发频率保护；
        // 分片睡眠让"停止重试"能在当前请求结束后立即生效
        let mut remaining_delay = archive_page_delay_ms(interval_ms);
        while remaining_delay > 0 {
            if archive.batch_cancel.load(Ordering::Relaxed) {
                break;
            }
            let slice = remaining_delay.min(200);
            tokio::time::sleep(std::time::Duration::from_millis(slice)).await;
            remaining_delay -= slice;
        }
    }
    archive.batch_retrying.store(false, Ordering::Relaxed);
    archive.batch_cancel.store(false, Ordering::Relaxed);
    set_progress(&archive, |progress| {
        progress.batch_retry = None;
    });
    Ok(result)
}

async fn retry_single_skip(
    app: &tauri::AppHandle,
    login: &tauri::State<'_, QLoginState>,
    archive: &tauri::State<'_, ArchiveState>,
    owner_uin: &str,
    id: i64,
) -> Result<ArchiveSkipRetryResult, String> {
    let connection = open_database(app)?;
    let (cursor, resolved_at) = connection
        .query_row(
            "SELECT cursor,resolved_at FROM archive_skips WHERE id=?1 AND owner_uin=?2",
            params![id, owner_uin],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => "找不到这条异常跳过记录".into(),
            _ => format!("读取异常跳过记录失败：{error}"),
        })?;
    if resolved_at.is_some() {
        return Ok(ArchiveSkipRetryResult {
            success: true,
            message: "该异常位置已经重试成功".into(),
            recovered_records: 0,
        });
    }
    if let Some(retry_at) = reserve_archive_page(app, owner_uin)? {
        return Err(format!("请求频率保护中，请在 {retry_at} 后重试"));
    }
    let attempted_at = now();
    match qzone::fetch_feeds_persistent(&login, "2", Some(&cursor)).await {
        Ok(page) => {
            let recovered_records = page.feeds.len() as u64;
            save_retried_page(app, owner_uin, &page.feeds)?;
            let connection = open_database(app)?;
            connection
                .execute(
                    "UPDATE archive_skips SET retry_count=retry_count+1,last_retry_at=?2,
                  resolved_at=?2,recovered_records=?3 WHERE id=?1 AND owner_uin=?4",
                    params![id, attempted_at, recovered_records, owner_uin],
                )
                .map_err(|error| format!("更新异常重试结果失败：{error}"))?;
            let remaining = unresolved_skip_count(app, owner_uin)?;
            set_progress(archive, |progress| progress.skipped = remaining);
            Ok(ArchiveSkipRetryResult {
                success: true,
                message: format!("重试成功，已恢复 {recovered_records} 条接口记录"),
                recovered_records,
            })
        }
        Err(error) => {
            let summary = concise_archive_error(&error);
            let connection = open_database(app)?;
            connection
                .execute(
                    "UPDATE archive_skips SET retry_count=retry_count+1,last_retry_at=?2,error=?3
                 WHERE id=?1 AND owner_uin=?4",
                    params![id, attempted_at, summary, owner_uin],
                )
                .map_err(|reason| format!("保存异常重试失败结果失败：{reason}"))?;
            Ok(ArchiveSkipRetryResult {
                success: false,
                message: format!("重试仍然失败：{summary}"),
                recovered_records: 0,
            })
        }
    }
}

#[tauri::command]
pub fn cancel_feed_archive(state: tauri::State<'_, ArchiveState>) {
    state.cancel.store(true, Ordering::Relaxed);
    state.batch_cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub async fn list_archived_feeds(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    limit: u32,
    offset: u32,
    category: String,
) -> Result<Vec<ArchiveItem>, String> {
    validate_category(&category)?;
    let owner_uin = login.qzone_auth().await?.uin;
    tauri::async_runtime::spawn_blocking(move || {
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT d.id,d.owner_uin,d.cell_id,d.published_at,d.content,d.author_uin,d.author_name,d.pictures_json,d.video_json,
              (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type=217),
              (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type IN (2,311))
             FROM archive_dynamics d WHERE d.owner_uin=?1 AND d.category=?2 ORDER BY d.published_at ASC LIMIT ?3 OFFSET ?4",
        )
        .map_err(|error| format!("读取归档失败：{error}"))?;
    let rows = statement
        .query_map(
            params![owner_uin, category, limit.clamp(1, 200), offset],
            |row| {
                let video_json = row.get::<_, Option<String>>(8)?;
                let video_urls = video_urls(video_json.clone());
                Ok(ArchiveItem {
                    id: row.get(0)?,
                    owner_uin: row.get(1)?,
                    cell_id: row.get(2)?,
                    published_at: row.get(3)?,
                    content: row.get(4)?,
                    author_uin: row.get(5)?,
                    author_name: row.get(6)?,
                    picture_urls: picture_urls(row.get(7)?),
                    video_url: video_urls.first().cloned(),
                    video_urls,
                    video_cover_url: video_cover_url(video_json),
                    like_count: row.get(9)?,
                    comment_count: row.get(10)?,
                    likes: vec![],
                    comments: vec![],
                })
            },
        )
        .map_err(|error| format!("查询归档失败：{error}"))?;
    let mut items = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取归档记录失败：{error}"))?;
    drop(statement);
    let mut comment_statement = connection
        .prepare(
            "SELECT comments_json,actor_uin,actor_name,event_summary,event_time FROM archive_feeds
         WHERE owner_uin=?1 AND cell_id=?2 AND event_type IN (2,311) ORDER BY event_time ASC",
        )
        .map_err(|error| format!("准备评论查询失败：{error}"))?;
    for item in &mut items {
        let comments = comment_statement
            .query_map(params![item.owner_uin, item.cell_id], |row| {
                Ok(comment_from_values(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|error| format!("查询动态评论失败：{error}"))?;
        item.comments = merge_comments(comments.filter_map(Result::ok));
    }
    drop(comment_statement);
    let mut like_statement = connection
        .prepare(
            "SELECT actor_uin,actor_name FROM archive_feeds
         WHERE owner_uin=?1 AND cell_id=?2 AND event_type=217 ORDER BY event_time ASC",
        )
        .map_err(|error| format!("准备点赞查询失败：{error}"))?;
    for item in &mut items {
        let likes = like_statement
            .query_map(params![item.owner_uin, item.cell_id], |row| {
                Ok(LikeUser {
                    uin: row.get(0)?,
                    nickname: row.get(1)?,
                })
            })
            .map_err(|error| format!("查询点赞用户失败：{error}"))?;
        item.likes = likes.filter_map(Result::ok).collect();
    }
    Ok(items)
    })
    .await
    .map_err(|error| format!("归档查询任务异常退出：{error}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    kind: String,
    ref_id: i64,
    cell_id: Option<String>,
    title: Option<String>,
    snippet: String,
    author_name: Option<String>,
    created_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    items: Vec<ArchiveItem>,
    hits: Vec<SearchHit>,
    total: u32,
}

/// 全局全文搜索：跨当前账号全部动态，匹配正文与作者昵称。
/// 优先走 FTS5 虚拟表，库未建索引时回退为 LIKE 子串匹配。
#[tauri::command]
pub async fn global_search(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    keyword: String,
    limit: u32,
    offset: u32,
) -> Result<SearchResult, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let keyword = keyword.trim().to_string();
    if keyword.is_empty() {
        return Err("搜索关键词不能为空".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        search_all(&app, &owner_uin, &keyword, limit, offset)
    })
    .await
    .map_err(|error| format!("搜索任务异常退出：{error}"))?
}

fn search_all(
    app: &tauri::AppHandle,
    owner_uin: &str,
    keyword: &str,
    limit: u32,
    offset: u32,
) -> Result<SearchResult, String> {
    let connection = open_database(app)?;
    let has_fts: bool = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='archive_search'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false);

    // FTS5 前缀匹配：每个有效词转 "词"*，空格连接的词默认 AND。
    // 纯符号关键词过滤后为空，回退 LIKE 子串匹配，避免 FTS 语法错误。
    let mut matcher = String::new();
    for token in keyword
        .split_whitespace()
        .filter(|token| token.chars().any(|character| character.is_alphanumeric()))
    {
        if !matcher.is_empty() {
            matcher.push(' ');
        }
        matcher.push('"');
        matcher.push_str(&token.replace('"', ""));
        matcher.push_str("\"*");
    }
    let use_fts = has_fts && !matcher.is_empty();

    let (rows_sql, params_vals, count_sql, count_params): (
        String,
        Vec<String>,
        String,
        Vec<String>,
    ) = if use_fts {
        (
                "SELECT d.id,d.owner_uin,d.cell_id,d.published_at,d.content,d.author_uin,d.author_name,d.pictures_json,d.video_json,
                   (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type=217),
                   (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type IN (2,311))
                  FROM archive_search s
                  JOIN archive_dynamics d ON d.id = s.rowid
                  WHERE archive_search MATCH ?1 AND s.owner_uin=?2
                  ORDER BY rank, d.published_at DESC LIMIT ?3 OFFSET ?4"
                    .to_string(),
                vec![matcher.clone(), owner_uin.to_string(), limit.to_string(), offset.to_string()],
                "SELECT COUNT(*) FROM archive_search s WHERE archive_search MATCH ?1 AND s.owner_uin=?2".to_string(),
                vec![matcher, owner_uin.to_string()],
            )
    } else {
        let like = format!("%{keyword}%");
        (
                "SELECT id,owner_uin,cell_id,published_at,content,author_uin,author_name,pictures_json,video_json,
                   0,0 FROM archive_dynamics
                   WHERE owner_uin=?1 AND (content LIKE ?2 OR author_name LIKE ?2)
                   ORDER BY published_at DESC LIMIT ?3 OFFSET ?4"
                    .to_string(),
                vec![owner_uin.to_string(), like.clone(), limit.to_string(), offset.to_string()],
                "SELECT COUNT(*) FROM archive_dynamics WHERE owner_uin=?1 AND (content LIKE ?2 OR author_name LIKE ?2)".to_string(),
                vec![owner_uin.to_string(), like],
            )
    };

    let _dynamic_total: u32 = {
        let mut stmt = connection
            .prepare(&count_sql)
            .map_err(|error| format!("准备搜索计数失败：{error}"))?;
        let mut rows = stmt
            .query(rusqlite::params_from_iter(count_params.iter()))
            .map_err(|error| format!("搜索计数失败：{error}"))?;
        let count = rows
            .next()
            .map_err(|error| format!("搜索计数失败：{error}"))?;
        count.and_then(|row| row.get(0).ok()).unwrap_or(0)
    };

    let items = {
        let mut statement = connection
            .prepare(&rows_sql)
            .map_err(|error| format!("准备搜索查询失败：{error}"))?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(params_vals.iter()), |row| {
                let video_json = row.get::<_, Option<String>>(8)?;
                let video_urls = video_urls(video_json.clone());
                Ok(ArchiveItem {
                    id: row.get(0)?,
                    owner_uin: row.get(1)?,
                    cell_id: row.get(2)?,
                    published_at: row.get(3)?,
                    content: row.get(4)?,
                    author_uin: row.get(5)?,
                    author_name: row.get(6)?,
                    picture_urls: picture_urls(row.get(7)?),
                    video_url: video_urls.first().cloned(),
                    video_urls,
                    video_cover_url: video_cover_url(video_json),
                    like_count: row.get(9)?,
                    comment_count: row.get(10)?,
                    likes: vec![],
                    comments: vec![],
                })
            })
            .map_err(|error| format!("执行搜索失败：{error}"))?;
        let mut items = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取搜索结果失败：{error}"))?;

        // 补充评论与点赞
        let mut comment_statement = connection
            .prepare(
                "SELECT comments_json,actor_uin,actor_name,event_summary,event_time FROM archive_feeds
             WHERE owner_uin=?1 AND cell_id=?2 AND event_type IN (2,311) ORDER BY event_time ASC",
            )
            .map_err(|error| format!("准备评论查询失败：{error}"))?;
        for item in &mut items {
            let comments = comment_statement
                .query_map(params![item.owner_uin, item.cell_id], |row| {
                    Ok(comment_from_values(
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .map_err(|error| format!("查询动态评论失败：{error}"))?;
            item.comments = merge_comments(comments.filter_map(Result::ok));
        }
        drop(comment_statement);
        let mut like_statement = connection
            .prepare(
                "SELECT actor_uin,actor_name FROM archive_feeds
             WHERE owner_uin=?1 AND cell_id=?2 AND event_type=217 ORDER BY event_time ASC",
            )
            .map_err(|error| format!("准备点赞查询失败：{error}"))?;
        for item in &mut items {
            let likes = like_statement
                .query_map(params![item.owner_uin, item.cell_id], |row| {
                    Ok(LikeUser {
                        uin: row.get(0)?,
                        nickname: row.get(1)?,
                    })
                })
                .map_err(|error| format!("查询点赞用户失败：{error}"))?;
            item.likes = likes.filter_map(Result::ok).collect();
        }
        items
    };

    let (hits, global_total) =
        search_global_documents(&connection, owner_uin, keyword, limit, offset)?;
    Ok(SearchResult {
        items,
        hits,
        total: global_total,
    })
}

fn search_global_documents(
    connection: &Connection,
    owner_uin: &str,
    keyword: &str,
    limit: u32,
    offset: u32,
) -> Result<(Vec<SearchHit>, u32), String> {
    let matcher = keyword
        .split_whitespace()
        .filter(|token| token.chars().any(|character| character.is_alphanumeric()))
        .map(|token| format!("\"{}\"*", token.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ");
    let use_fts = !matcher.is_empty() && keyword.is_ascii();
    let limit = limit.clamp(1, 100);

    let (total, rows) = if use_fts {
        let total: u32 = connection
            .query_row(
                "SELECT COUNT(*) FROM archive_global_search s
                 WHERE archive_global_search MATCH ?1 AND s.owner_uin=?2",
                params![matcher, owner_uin],
                |row| row.get(0),
            )
            .map_err(|error| format!("全局搜索计数失败：{error}"))?;
        let mut statement = connection
            .prepare(
                "SELECT d.kind,d.ref_id,d.cell_id,d.title,d.body,d.author_name,d.created_at
                 FROM archive_global_search s
                 JOIN archive_search_documents d ON d.id=s.rowid
                 WHERE archive_global_search MATCH ?1 AND s.owner_uin=?2
                 ORDER BY rank,d.created_at DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|error| format!("准备全局搜索失败：{error}"))?;
        let rows = statement
            .query_map(
                params![matcher, owner_uin, limit, offset],
                search_hit_from_row,
            )
            .map_err(|error| format!("执行全局搜索失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取全局搜索结果失败：{error}"))?;
        (total, rows)
    } else {
        let like = format!("%{keyword}%");
        let total: u32 = connection
            .query_row(
                "SELECT COUNT(*) FROM archive_search_documents
                 WHERE owner_uin=?1 AND (COALESCE(title,'') LIKE ?2 OR COALESCE(body,'') LIKE ?2 OR COALESCE(author_name,'') LIKE ?2)",
                params![owner_uin, like],
                |row| row.get(0),
            )
            .map_err(|error| format!("全局搜索计数失败：{error}"))?;
        let mut statement = connection
            .prepare(
                "SELECT kind,ref_id,cell_id,title,body,author_name,created_at
                 FROM archive_search_documents
                 WHERE owner_uin=?1 AND (COALESCE(title,'') LIKE ?2 OR COALESCE(body,'') LIKE ?2 OR COALESCE(author_name,'') LIKE ?2)
                 ORDER BY created_at DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|error| format!("准备全局搜索失败：{error}"))?;
        let rows = statement
            .query_map(params![owner_uin, like, limit, offset], search_hit_from_row)
            .map_err(|error| format!("执行全局搜索失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取全局搜索结果失败：{error}"))?;
        (total, rows)
    };
    Ok((rows, total))
}

fn search_hit_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    let title: Option<String> = row.get(3)?;
    let body: Option<String> = row.get(4)?;
    let snippet = body.clone().or(title.clone()).unwrap_or_default();
    let snippet = snippet.chars().take(160).collect();
    Ok(SearchHit {
        kind: row.get(0)?,
        ref_id: row.get(1)?,
        cell_id: row.get(2)?,
        title,
        snippet,
        author_name: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// 那年今日：查询历史上今天（相同月日，但不同年份）的动态记录
#[tauri::command]
pub async fn list_on_this_day_feeds(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<Vec<ArchiveItem>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    tauri::async_runtime::spawn_blocking(move || {
        let connection = open_database(&app)?;
        // 使用 sqlite 的 strftime('%m-%d', datetime(published_at, 'unixepoch', 'localtime')) 进行对比
        let mut statement = connection
            .prepare(
                "SELECT d.id,d.owner_uin,d.cell_id,d.published_at,d.content,d.author_uin,d.author_name,d.pictures_json,d.video_json,
                  (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type=217),
                  (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type IN (2,311))
                 FROM archive_dynamics d
                 WHERE d.owner_uin=?1 AND d.published_at > 0
                   AND strftime('%m-%d', datetime(d.published_at, 'unixepoch', 'localtime')) = strftime('%m-%d', 'now', 'localtime')
                   AND strftime('%Y', datetime(d.published_at, 'unixepoch', 'localtime')) != strftime('%Y', 'now', 'localtime')
                 ORDER BY d.published_at DESC LIMIT 50",
            )
            .map_err(|error| format!("准备那年今日查询失败：{error}"))?;
        let rows = statement
            .query_map(params![owner_uin], |row| {
                let video_json = row.get::<_, Option<String>>(8)?;
                let video_urls = video_urls(video_json.clone());
                Ok(ArchiveItem {
                    id: row.get(0)?,
                    owner_uin: row.get(1)?,
                    cell_id: row.get(2)?,
                    published_at: row.get(3)?,
                    content: row.get(4)?,
                    author_uin: row.get(5)?,
                    author_name: row.get(6)?,
                    picture_urls: picture_urls(row.get(7)?),
                    video_url: video_urls.first().cloned(),
                    video_urls,
                    video_cover_url: video_cover_url(video_json),
                    like_count: row.get(9)?,
                    comment_count: row.get(10)?,
                    likes: vec![],
                    comments: vec![],
                })
            })
            .map_err(|error| format!("查询那年今日失败：{error}"))?;
        let items = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取那年今日记录失败：{error}"))?;
        Ok(items)
    })
    .await
    .map_err(|error| format!("那年今日任务异常退出：{error}"))?
}

#[tauri::command]
pub async fn list_archived_media(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    limit: u32,
    offset: u32,
    year: Option<i32>,
) -> Result<ArchiveMediaPage, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let mut year_statement = connection.prepare(
        "SELECT DISTINCT CAST(strftime('%Y',published_at,'unixepoch','localtime') AS INTEGER) FROM archive_dynamics
         WHERE owner_uin=?1 AND category IN ('self','other') AND (pictures_json IS NOT NULL OR video_json IS NOT NULL)
         ORDER BY 1 DESC",
    ).map_err(|error| format!("读取媒体年份失败：{error}"))?;
    let years = year_statement
        .query_map(params![owner_uin], |row| row.get::<_, i32>(0))
        .map_err(|error| format!("查询媒体年份失败：{error}"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    drop(year_statement);

    let mut statement = connection.prepare(
        "SELECT id,published_at,content,author_uin,author_name,pictures_json,video_json FROM archive_dynamics
         WHERE owner_uin=?1 AND category IN ('self','other')
           AND (pictures_json IS NOT NULL OR video_json IS NOT NULL)
           AND (?2 IS NULL OR CAST(strftime('%Y',published_at,'unixepoch','localtime') AS INTEGER)=?2)
         ORDER BY published_at ASC,id ASC",
    ).map_err(|error| format!("读取媒体归档失败：{error}"))?;
    let rows = statement
        .query_map(params![owner_uin, year], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|error| format!("查询媒体归档失败：{error}"))?;
    let mut all = Vec::new();
    for row in rows {
        let (id, published_at, content, author_uin, author_name, pictures_json, video_json) =
            row.map_err(|error| format!("读取媒体记录失败：{error}"))?;
        for (index, url) in picture_urls(pictures_json).into_iter().enumerate() {
            all.push(ArchiveMediaItem {
                key: format!("{id}-photo-{index}"),
                dynamic_id: id,
                media_type: "photo",
                picture_index: Some(index),
                url,
                cover_url: None,
                published_at,
                author_uin: author_uin.clone(),
                author_name: author_name.clone(),
                content: content.clone(),
            });
        }
        let videos = video_urls(video_json.clone());
        if let Some(url) = videos.first() {
            all.push(ArchiveMediaItem {
                key: format!("{id}-video"),
                dynamic_id: id,
                media_type: "video",
                picture_index: None,
                url: url.clone(),
                cover_url: video_cover_url(video_json),
                published_at,
                author_uin,
                author_name,
                content,
            });
        }
    }
    let total = all.len();
    let start = (offset as usize).min(total);
    let end = (start + limit.clamp(1, 100) as usize).min(total);
    let items = all.drain(start..end).collect();
    Ok(ArchiveMediaPage {
        items,
        total,
        years,
    })
}

#[tauri::command]
pub async fn get_archived_feed(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    id: i64,
) -> Result<ArchiveItem, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let mut item = connection.query_row(
        "SELECT d.id,d.owner_uin,d.cell_id,d.published_at,d.content,d.author_uin,d.author_name,d.pictures_json,d.video_json,
          (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type=217),
          (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type IN (2,311))
         FROM archive_dynamics d WHERE d.owner_uin=?1 AND d.id=?2",
        params![owner_uin, id], |row| {
            let video_json = row.get::<_, Option<String>>(8)?;
            let video_urls = video_urls(video_json.clone());
            Ok(ArchiveItem { id: row.get(0)?, owner_uin: row.get(1)?, cell_id: row.get(2)?, published_at: row.get(3)?,
                content: row.get(4)?, author_uin: row.get(5)?, author_name: row.get(6)?, picture_urls: picture_urls(row.get(7)?),
                video_url: video_urls.first().cloned(), video_urls, video_cover_url: video_cover_url(video_json),
                like_count: row.get(9)?, comment_count: row.get(10)?, likes: vec![], comments: vec![] })
        },
    ).map_err(|error| match error { rusqlite::Error::QueryReturnedNoRows => "原始动态不存在或已删除".into(), _ => format!("读取原始动态失败：{error}") })?;
    let mut comments = connection
        .prepare(
            "SELECT comments_json,actor_uin,actor_name,event_summary,event_time FROM archive_feeds
         WHERE owner_uin=?1 AND cell_id=?2 AND event_type IN (2,311) ORDER BY event_time ASC",
        )
        .map_err(|error| format!("准备评论查询失败：{error}"))?;
    item.comments = merge_comments(
        comments
            .query_map(params![item.owner_uin, item.cell_id], |row| {
                Ok(comment_from_values(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|error| format!("查询动态评论失败：{error}"))?
            .filter_map(Result::ok),
    );
    drop(comments);
    let mut likes_stmt = connection
        .prepare(
            "SELECT actor_uin,actor_name FROM archive_feeds
         WHERE owner_uin=?1 AND cell_id=?2 AND event_type=217 ORDER BY event_time ASC",
        )
        .map_err(|error| format!("准备点赞查询失败：{error}"))?;
    item.likes = likes_stmt
        .query_map(params![item.owner_uin, item.cell_id], |row| {
            Ok(LikeUser {
                uin: row.get(0)?,
                nickname: row.get(1)?,
            })
        })
        .map_err(|error| format!("查询点赞用户失败：{error}"))?
        .filter_map(Result::ok)
        .collect();
    Ok(item)
}

fn comment_from_values(
    json: Option<String>,
    fallback_uin: Option<String>,
    fallback_name: Option<String>,
    fallback_content: Option<String>,
    fallback_time: i64,
) -> ArchiveComment {
    let value = json.and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let main = value.as_ref().and_then(|value| value.get("main_comment"));
    let comment_id = main.and_then(|value| text_at(value, "/commentid"));
    let main_uin = main.and_then(|value| text_at(value, "/user/uin"));
    let main_name = main.and_then(|value| text_at(value, "/user/nickname"));
    let main_content = main.and_then(|value| text_at(value, "/content"));
    let main_time = main
        .and_then(|value| value.get("date"))
        .and_then(Value::as_i64)
        .unwrap_or(fallback_time);
    let mut replies: Vec<ArchiveReply> = main
        .and_then(|value| value.get("replys"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(reply_from_value)
        .collect();
    if let Some(comment_id) = comment_id.as_deref() {
        let related_replies = value
            .as_ref()
            .and_then(|value| value.get("comments"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|comment| text_at(comment, "/commentid").as_deref() == Some(comment_id))
            .filter_map(|comment| comment.get("replys"))
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(reply_from_value);
        for reply in related_replies {
            let duplicate = replies.iter().any(|candidate| {
                candidate.uin == reply.uin
                    && candidate.content == reply.content
                    && candidate.created_at == reply.created_at
            });
            if !duplicate {
                replies.push(reply);
            }
        }
    }

    // Reply notifications keep the parent in main_comment but put the actual
    // reply text and author at the feed level. When the parent author replies
    // again, target the latest preceding reply from the other participant.
    let is_reply_notification = main
        .and_then(|value| value.get("replynum"))
        .and_then(Value::as_i64)
        .is_some_and(|count| count > 0)
        && main_uin.is_some()
        && fallback_uin.is_some()
        && main_content.as_deref() != fallback_content.as_deref()
        && fallback_time > main_time;
    if is_reply_notification {
        if let Some(content) = fallback_content.clone() {
            let duplicate = replies
                .iter()
                .any(|reply| reply.uin == fallback_uin && reply.content == content);
            if !duplicate {
                let reply_target = replies
                    .iter()
                    .filter(|reply| reply.uin != fallback_uin && reply.created_at <= fallback_time)
                    .max_by_key(|reply| reply.created_at);
                replies.push(ArchiveReply {
                    uin: fallback_uin.clone(),
                    nickname: fallback_name.clone(),
                    reply_to_uin: reply_target
                        .and_then(|reply| reply.uin.clone())
                        .or_else(|| main_uin.clone()),
                    reply_to_nickname: reply_target
                        .and_then(|reply| reply.nickname.clone())
                        .or_else(|| main_name.clone()),
                    content,
                    created_at: fallback_time,
                });
            }
        }
    }

    ArchiveComment {
        comment_id,
        uin: main_uin.or(fallback_uin),
        nickname: main_name.or(fallback_name),
        content: main_content
            .or(fallback_content)
            .unwrap_or_else(|| "评论了这条动态".into()),
        created_at: main_time,
        replies,
    }
}

fn reply_from_value(value: &Value) -> Option<ArchiveReply> {
    let content = text_at(value, "/content")?;
    Some(ArchiveReply {
        uin: text_at(value, "/user/uin").or_else(|| text_at(value, "/replyuser/uin")),
        nickname: text_at(value, "/user/nickname")
            .or_else(|| text_at(value, "/replyuser/nickname")),
        reply_to_uin: text_at(value, "/replyuser/uin")
            .or_else(|| text_at(value, "/targetuser/uin"))
            .or_else(|| text_at(value, "/target/uin")),
        reply_to_nickname: text_at(value, "/replyuser/nickname")
            .or_else(|| text_at(value, "/targetuser/nickname"))
            .or_else(|| text_at(value, "/target/nickname")),
        content,
        created_at: value.get("date").and_then(Value::as_i64).unwrap_or(0),
    })
}

fn merge_comments(comments: impl IntoIterator<Item = ArchiveComment>) -> Vec<ArchiveComment> {
    let mut merged: Vec<ArchiveComment> = Vec::new();
    for mut comment in comments {
        let existing = merged.iter_mut().find(|candidate| {
            (comment.comment_id.is_some() && candidate.comment_id == comment.comment_id)
                || (candidate.uin == comment.uin
                    && candidate.content == comment.content
                    && candidate.created_at == comment.created_at)
        });
        if let Some(existing) = existing {
            for reply in comment.replies.drain(..) {
                let duplicate = existing.replies.iter().any(|candidate| {
                    candidate.uin == reply.uin
                        && candidate.content == reply.content
                        && candidate.created_at == reply.created_at
                });
                if !duplicate {
                    existing.replies.push(reply);
                }
            }
            existing.replies.sort_by_key(|reply| reply.created_at);
        } else {
            comment.replies.sort_by_key(|reply| reply.created_at);
            merged.push(comment);
        }
    }
    merged
}

pub(crate) fn validate_category(category: &str) -> Result<(), String> {
    match category {
        "self" | "other" | "guestbook" => Ok(()),
        _ => Err("无效的归档分类".into()),
    }
}

pub(crate) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn mention_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"@\{uin:([^,}]+),nick:([^}]+)\}").expect("fixed mention regex")
    })
}

fn qzone_text_html(value: Option<&str>) -> String {
    let text = value
        .unwrap_or("")
        .trim_start_matches(['：', ':'])
        .trim_start();
    let pattern = mention_pattern();
    let mut html = String::new();
    let mut cursor = 0;
    for captures in pattern.captures_iter(text) {
        let matched = captures.get(0).expect("full capture");
        html.push_str(&html_escape(&text[cursor..matched.start()]));
        html.push_str("<span class=\"mention\" title=\"QQ ");
        html.push_str(&html_escape(&captures[1]));
        html.push_str("\">@");
        html.push_str(&html_escape(&captures[2]));
        html.push_str("</span>");
        cursor = matched.end();
    }
    html.push_str(&html_escape(&text[cursor..]));
    if html.is_empty() {
        "<span class=\"muted\">该动态没有文字内容</span>".into()
    } else {
        html
    }
}

pub(crate) fn archive_items_for_export(
    connection: &Connection,
    owner_uin: &str,
    category: &str,
    selected_ids: Option<&HashSet<i64>>,
) -> Result<Vec<ArchiveItem>, String> {
    archive_items_for_export_since(connection, owner_uin, category, selected_ids, None)
}

pub(crate) fn archive_items_for_export_since(
    connection: &Connection,
    owner_uin: &str,
    category: &str,
    selected_ids: Option<&HashSet<i64>>,
    since: Option<i64>,
) -> Result<Vec<ArchiveItem>, String> {
    let mut statement = connection.prepare(
        "SELECT d.id,d.owner_uin,d.cell_id,d.published_at,d.content,d.author_uin,d.author_name,d.pictures_json,d.video_json,
          (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type=217),
          (SELECT COUNT(*) FROM archive_feeds f WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_type IN (2,311))
         FROM archive_dynamics d WHERE d.owner_uin=?1 AND d.category=?2
           AND (?3 IS NULL OR d.archived_at >= ?3 OR EXISTS (
               SELECT 1 FROM archive_feeds f
               WHERE f.owner_uin=d.owner_uin AND f.cell_id=d.cell_id AND f.event_time >= ?3
           ))
         ORDER BY d.published_at ASC"
    ).map_err(|error| format!("准备导出查询失败：{error}"))?;
    let rows = statement
        .query_map(params![owner_uin, category, since], |row| {
            let video_json = row.get::<_, Option<String>>(8)?;
            let video_urls = video_urls(video_json.clone());
            Ok(ArchiveItem {
                id: row.get(0)?,
                owner_uin: row.get(1)?,
                cell_id: row.get(2)?,
                published_at: row.get(3)?,
                content: row.get(4)?,
                author_uin: row.get(5)?,
                author_name: row.get(6)?,
                picture_urls: picture_urls(row.get(7)?),
                video_url: video_urls.first().cloned(),
                video_urls,
                video_cover_url: video_cover_url(video_json),
                like_count: row.get(9)?,
                comment_count: row.get(10)?,
                likes: vec![],
                comments: vec![],
            })
        })
        .map_err(|error| format!("查询导出内容失败：{error}"))?;
    let mut items = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取导出内容失败：{error}"))?;
    if let Some(ids) = selected_ids {
        items.retain(|item| ids.contains(&item.id));
    }
    drop(statement);
    let mut comments = connection
        .prepare(
            "SELECT comments_json,actor_uin,actor_name,event_summary,event_time FROM archive_feeds
         WHERE owner_uin=?1 AND cell_id=?2 AND event_type IN (2,311) ORDER BY event_time ASC",
        )
        .map_err(|error| format!("准备导出评论失败：{error}"))?;
    for item in &mut items {
        let rows = comments
            .query_map(params![item.owner_uin, item.cell_id], |row| {
                Ok(comment_from_values(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|error| format!("查询导出评论失败：{error}"))?;
        item.comments = merge_comments(rows.filter_map(Result::ok));
    }
    drop(comments);
    let mut export_likes = connection
        .prepare(
            "SELECT actor_uin,actor_name FROM archive_feeds
         WHERE owner_uin=?1 AND cell_id=?2 AND event_type=217 ORDER BY event_time ASC",
        )
        .map_err(|error| format!("准备导出点赞查询失败：{error}"))?;
    for item in &mut items {
        let likes = export_likes
            .query_map(params![item.owner_uin, item.cell_id], |row| {
                Ok(LikeUser {
                    uin: row.get(0)?,
                    nickname: row.get(1)?,
                })
            })
            .map_err(|error| format!("查询导出点赞用户失败：{error}"))?;
        item.likes = likes.filter_map(Result::ok).collect();
    }
    Ok(items)
}

/// 导出 HTML 的媒体地址解析器：把动态里的图片/视频改写为实际展示地址。
/// 轻量导出直接使用远程地址；离线包导出改写为包内相对路径。
pub(crate) struct ExportMediaResolver<'a> {
    /// (动态下标, 图片下标, 远程地址) -> 展示地址
    pub picture: &'a dyn Fn(usize, usize, &str) -> String,
    /// (动态下标, 远程视频地址) -> 包内视频地址；None 表示保留远程链接
    pub video: &'a dyn Fn(usize, &str) -> Option<String>,
}

pub(crate) const EXPORT_CSS: &str = "*{box-sizing:border-box}body{margin:0;background:#f3f6fb;color:#243247;font:14px/1.7 system-ui,-apple-system,\"Microsoft YaHei\",sans-serif}main{width:min(820px,calc(100% - 24px));margin:30px auto}h1{margin:0}.intro{color:#758298;margin:0 0 20px}.card{background:#fff;border:1px solid #e5eaf2;border-radius:16px;padding:20px;margin:14px 0;box-shadow:0 8px 25px #2038580b}header{display:flex;gap:11px;align-items:center}.avatar{width:44px;height:44px;border-radius:50%}header strong,header small{display:block}small,.muted,.stats{color:#7e899a}.content{margin:14px 0;white-space:pre-wrap;overflow-wrap:anywhere}.mention,a{color:#2684ff}.pictures{display:grid;grid-template-columns:repeat(3,1fr);gap:6px}.pictures img{display:block;width:100%;height:210px;object-fit:cover;border-radius:8px}.video{display:inline-block;padding:7px 12px;background:#edf5ff;border-radius:9px;text-decoration:none}.stats{margin-top:12px}.comments{margin-top:12px;padding:12px;background:#f6f8fb;border-radius:10px}.comment{margin:8px 0}.comment-meta{margin-bottom:3px;color:#7e899a;font-size:11px}.comment-meta b{color:#2684ff}.replies{margin:6px 0 0 18px;padding:7px 10px;border-left:2px solid #c9dcf6;background:#fff;border-radius:0 7px 7px 0}@media(max-width:600px){main{margin:16px auto}.card{padding:15px}.pictures img{height:125px}}";

pub(crate) fn render_feed_cards(items: &[ArchiveItem], media: &ExportMediaResolver<'_>) -> String {
    let mut cards = String::new();
    for (item_index, item) in items.iter().enumerate() {
        let author = item
            .author_name
            .as_deref()
            .or(item.author_uin.as_deref())
            .unwrap_or("QQ 用户");
        cards.push_str("<article class=\"card\"><header><img class=\"avatar\" src=\"https://qlogo2.store.qq.com/qzone/");
        let uin = item.author_uin.as_deref().unwrap_or("0");
        cards.push_str(&html_escape(uin));
        cards.push('/');
        cards.push_str(&html_escape(uin));
        cards.push_str("/50\"><div><strong>");
        cards.push_str(&html_escape(author));
        cards.push_str("</strong><small>");
        if let Some(author_uin) = &item.author_uin {
            cards.push_str("QQ ");
            cards.push_str(&html_escape(author_uin));
            cards.push_str(" · ");
        }
        cards.push_str("<time data-time=\"");
        cards.push_str(&item.published_at.to_string());
        cards.push_str("\"></time></small></div></header><div class=\"content\">");
        cards.push_str(&qzone_text_html(item.content.as_deref()));
        cards.push_str("</div>");
        if !item.picture_urls.is_empty() {
            cards.push_str("<div class=\"pictures\">");
            for (picture_index, url) in item.picture_urls.iter().enumerate() {
                let src = (media.picture)(item_index, picture_index, url);
                cards.push_str("<a href=\"");
                cards.push_str(&html_escape(url));
                cards.push_str("\" target=\"_blank\"><img loading=\"lazy\" referrerpolicy=\"no-referrer\" src=\"");
                cards.push_str(&html_escape(&src));
                cards.push_str("\"></a>");
            }
            cards.push_str("</div>");
        }
        if let Some(video) = &item.video_url {
            match (media.video)(item_index, video) {
                Some(local) => {
                    cards.push_str("<video controls preload=\"metadata\" style=\"width:100%;max-width:480px;border-radius:9px;background:#000\" src=\"");
                    cards.push_str(&html_escape(&local));
                    cards.push_str("\"></video>");
                }
                None => {
                    cards.push_str("<p><a class=\"video\" href=\"");
                    cards.push_str(&html_escape(video));
                    cards.push_str("\" target=\"_blank\">▶ 查看视频</a></p>");
                }
            }
        }
        cards.push_str("<div class=\"stats\">");
        if !item.likes.is_empty() {
            cards.push_str("♥ ");
            let names: Vec<String> = item
                .likes
                .iter()
                .take(10)
                .map(|l| {
                    html_escape(
                        l.nickname
                            .as_deref()
                            .or(l.uin.as_deref())
                            .unwrap_or("QQ用户"),
                    )
                })
                .collect();
            cards.push_str(&names.join("、"));
            if item.likes.len() > 10 {
                cards.push_str(" 等 ");
                cards.push_str(&item.like_count.to_string());
                cards.push_str(" 人赞了");
            } else {
                cards.push_str(" 赞了");
            }
        }
        cards.push_str("　💬 ");
        cards.push_str(&item.comment_count.to_string());
        cards.push_str(" 条评论</div>");
        if !item.comments.is_empty() {
            cards.push_str("<section class=\"comments\">");
            for comment in &item.comments {
                let comment_name = comment
                    .nickname
                    .as_deref()
                    .or(comment.uin.as_deref())
                    .unwrap_or("QQ 用户");
                cards.push_str("<div class=\"comment\"><div class=\"comment-meta\"><b>");
                cards.push_str(&html_escape(comment_name));
                cards.push_str("</b> 评论于 <time data-time=\"");
                cards.push_str(&comment.created_at.to_string());
                cards.push_str("\"></time></div>");
                cards.push_str(&qzone_text_html(Some(&comment.content)));
                if !comment.replies.is_empty() {
                    cards.push_str("<div class=\"replies\">");
                    for reply in &comment.replies {
                        let reply_name = reply
                            .nickname
                            .as_deref()
                            .or(reply.uin.as_deref())
                            .unwrap_or("QQ 用户");
                        cards.push_str("<div><div class=\"comment-meta\"><b>");
                        cards.push_str(&html_escape(reply_name));
                        cards.push_str("</b> 回复 ");
                        cards.push_str(&html_escape(
                            reply
                                .reply_to_nickname
                                .as_deref()
                                .or(reply.reply_to_uin.as_deref())
                                .unwrap_or(comment_name),
                        ));
                        cards.push_str(" · <time data-time=\"");
                        cards.push_str(&reply.created_at.to_string());
                        cards.push_str("\"></time></div>");
                        cards.push_str(&qzone_text_html(Some(&reply.content)));
                        cards.push_str("</div>");
                    }
                    cards.push_str("</div>");
                }
                cards.push_str("</div>");
            }
            cards.push_str("</section>");
        }
        cards.push_str("</article>");
    }
    cards
}

pub(crate) fn feed_document(
    items: &[ArchiveItem],
    category_name: &str,
    owner_uin: &str,
    media: &ExportMediaResolver<'_>,
) -> String {
    let cards = render_feed_cards(items, media);
    format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>QQ空间归档 - {category_name}</title><style>{css}</style></head><body><main><h1>QQ空间归档 · {category_name}</h1><p class="intro">账号 {owner} · 共 {count} 条 · 导出时间 <span id="export-time"></span></p>{cards}</main><script>document.querySelector('#export-time').textContent=new Date().toLocaleString();document.querySelectorAll('time[data-time]').forEach(e=>e.textContent=new Date(Number(e.dataset.time)*1000).toLocaleString());</script></body></html>"#,
        css = EXPORT_CSS,
        category_name = html_escape(category_name),
        owner = html_escape(owner_uin),
        count = items.len(),
        cards = cards,
    )
}

#[tauri::command]
pub async fn export_archived_html(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    category: String,
    ids: Option<Vec<i64>>,
) -> Result<String, String> {
    validate_category(&category)?;
    let owner_uin = login.qzone_auth().await?.uin;
    let selected = ids.map(|values| values.into_iter().collect::<HashSet<_>>());
    if selected.as_ref().is_some_and(HashSet::is_empty) {
        return Err("请先选择需要导出的归档".into());
    }
    // 全量扫描 + HTML 渲染较重，移入阻塞线程池避免卡住异步运行时。
    tauri::async_runtime::spawn_blocking(move || {
        let connection = open_database(&app)?;
        let items =
            archive_items_for_export(&connection, &owner_uin, &category, selected.as_ref())?;
        if items.is_empty() {
            return Err("当前分类没有可以导出的归档".into());
        }
        let category_name = match category.as_str() {
            "self" => "本人动态",
            "other" => "其他动态",
            _ => "留言",
        };
        let resolver = ExportMediaResolver {
            picture: &|_, _, url| url.to_owned(),
            video: &|_, _| None,
        };
        Ok(feed_document(&items, category_name, &owner_uin, &resolver))
    })
    .await
    .map_err(|error| format!("归档导出任务异常退出：{error}"))?
}

#[tauri::command]
pub async fn count_archived_feeds(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    category: String,
) -> Result<u64, String> {
    validate_category(&category)?;
    let owner_uin = login.qzone_auth().await?.uin;
    tauri::async_runtime::spawn_blocking(move || {
        let connection = open_database(&app)?;
        connection
            .query_row(
                "SELECT COUNT(*) FROM archive_dynamics WHERE owner_uin=?1 AND category=?2",
                params![owner_uin, category],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as u64)
            .map_err(|error| format!("统计归档数量失败：{error}"))
    })
    .await
    .map_err(|error| format!("归档统计任务异常退出：{error}"))?
}

#[tauri::command]
pub async fn get_archive_overview(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<ArchiveOverview, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let database = database_path(&app)?;
    let connection = open_database(&app)?;
    let dynamics = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_dynamics WHERE owner_uin=?1",
            params![owner_uin],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("统计原动态失败：{error}"))?
        .max(0) as u64;
    let (likes, comments) = connection
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN event_type=217 THEN 1 ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN event_type IN (2,311) THEN 1 ELSE 0 END),0)
         FROM archive_feeds WHERE owner_uin=?1",
            params![owner_uin],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|error| format!("统计互动记录失败：{error}"))?;
    let mut statement = connection.prepare("SELECT pictures_json FROM archive_dynamics WHERE owner_uin=?1 AND pictures_json IS NOT NULL")
        .map_err(|error| format!("读取图片统计失败：{error}"))?;
    let pictures = statement
        .query_map(params![owner_uin], |row| row.get::<_, Option<String>>(0))
        .map_err(|error| format!("查询图片统计失败：{error}"))?
        .filter_map(Result::ok)
        .map(|json| picture_urls(json).len() as u64)
        .sum();
    let database_bytes = fs::metadata(database).map(|value| value.len()).unwrap_or(0);
    let albums = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_albums WHERE owner_uin=?1",
            params![owner_uin],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("统计相册失败：{error}"))?
        .max(0) as u64;
    let album_photos_backed = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_album_photos WHERE owner_uin=?1 AND status='done'",
            params![owner_uin],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("统计相册照片失败：{error}"))?
        .max(0) as u64;
    Ok(ArchiveOverview {
        dynamics,
        pictures,
        comments: comments.max(0) as u64,
        likes: likes.max(0) as u64,
        albums,
        album_photos_backed,
        database_bytes,
    })
}

fn health_message(integrity: &str, pending_skips: u64) -> (&'static str, bool) {
    if integrity != "ok" {
        return (
            "数据库完整性检查未通过，请先备份数据并停止继续归档。",
            false,
        );
    }
    if pending_skips > 0 {
        return ("数据库正常，仍有异常位置可在归档任务中重试。", true);
    }
    ("数据库正常，当前没有待重试异常。", true)
}

#[tauri::command]
pub async fn check_archive_health(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<ArchiveHealth, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let database = database_path(&app)?;
    let connection = open_database(&app)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| format!("检查归档数据库完整性失败：{error}"))?;
    let schema_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("读取归档数据库版本失败：{error}"))?;
    let dynamics: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_dynamics WHERE owner_uin=?1",
            params![owner_uin],
            |row| row.get(0),
        )
        .map_err(|error| format!("统计归档动态失败：{error}"))?;
    let interactions: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_feeds WHERE owner_uin=?1",
            params![owner_uin],
            |row| row.get(0),
        )
        .map_err(|error| format!("统计互动记录失败：{error}"))?;
    let pending_skips: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_skips WHERE owner_uin=?1 AND resolved_at IS NULL",
            params![owner_uin],
            |row| row.get(0),
        )
        .map_err(|error| format!("统计待重试异常失败：{error}"))?;
    let (message, healthy) = health_message(&integrity, pending_skips.max(0) as u64);
    Ok(ArchiveHealth {
        healthy,
        integrity,
        schema_version,
        dynamics: dynamics.max(0) as u64,
        interactions: interactions.max(0) as u64,
        pending_skips: pending_skips.max(0) as u64,
        database_bytes: fs::metadata(database).map(|value| value.len()).unwrap_or(0),
        message: message.into(),
    })
}

#[tauri::command]
pub async fn get_interaction_ranking(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    limit: u32,
) -> Result<Vec<InteractionRank>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT actor_uin,COALESCE(MAX(NULLIF(actor_name,'')),actor_uin),COUNT(*),
                SUM(CASE WHEN event_type=217 THEN 1 ELSE 0 END),
                SUM(CASE WHEN event_type IN (2,311) THEN 1 ELSE 0 END)
         FROM archive_feeds
         WHERE owner_uin=?1 AND actor_uin IS NOT NULL AND actor_uin<>'' AND actor_uin<>?1
           AND event_type IN (2,217,311)
         GROUP BY actor_uin
         ORDER BY COUNT(*) DESC,MAX(event_time) DESC
         LIMIT ?2",
        )
        .map_err(|error| format!("准备互动排行榜查询失败：{error}"))?;
    let rows = statement
        .query_map(params![owner_uin, limit.clamp(1, 50)], |row| {
            Ok(InteractionRank {
                uin: row.get(0)?,
                nickname: row.get(1)?,
                interactions: row.get::<_, i64>(2)?.max(0) as u64,
                likes: row.get::<_, i64>(3)?.max(0) as u64,
                comments: row.get::<_, i64>(4)?.max(0) as u64,
            })
        })
        .map_err(|error| format!("查询互动排行榜失败：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取互动排行榜失败：{error}"))
}

fn ensure_archive_idle(state: &ArchiveState) -> Result<(), String> {
    let progress = state.progress.lock().map_err(|_| "归档状态锁已损坏")?;
    if progress.status == "running" {
        return Err("归档任务运行时不能删除数据，请先取消任务".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_archived_feeds(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    state: tauri::State<'_, ArchiveState>,
    ids: Vec<i64>,
) -> Result<u64, String> {
    ensure_archive_idle(&state)?;
    let owner_uin = login.qzone_auth().await?.uin;
    if ids.is_empty() {
        return Ok(0);
    }
    if ids.len() > 500 {
        return Err("单次最多删除 500 条归档记录".into());
    }
    let mut connection = open_database(&app)?;
    let transaction = connection
        .transaction()
        .map_err(|error| format!("开始删除事务失败：{error}"))?;
    let mut count = 0;
    for id in ids {
        transaction.execute(
            "DELETE FROM archive_feeds WHERE owner_uin=?1 AND cell_id=(SELECT cell_id FROM archive_dynamics WHERE id=?2 AND owner_uin=?1)",
            params![owner_uin, id],
        ).map_err(|error| format!("删除动态互动失败：{error}"))?;
        count += transaction
            .execute(
                "DELETE FROM archive_dynamics WHERE id=?1 AND owner_uin=?2",
                params![id, owner_uin],
            )
            .map_err(|error| format!("批量删除归档失败：{error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("提交删除事务失败：{error}"))?;
    Ok(count as u64)
}

#[tauri::command]
pub async fn clear_archived_feeds(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    state: tauri::State<'_, ArchiveState>,
) -> Result<u64, String> {
    ensure_archive_idle(&state)?;
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let dynamics = connection
        .execute(
            "DELETE FROM archive_dynamics WHERE owner_uin=?1",
            params![owner_uin],
        )
        .map_err(|error| format!("清空原动态失败：{error}"))?;
    connection
        .execute(
            "DELETE FROM archive_feeds WHERE owner_uin=?1",
            params![owner_uin],
        )
        .map_err(|error| format!("清空互动记录失败：{error}"))?;
    connection
        .execute(
            "DELETE FROM archive_checkpoints WHERE owner_uin=?1",
            params![owner_uin],
        )
        .map_err(|error| format!("清空归档续传位置失败：{error}"))?;
    connection
        .execute(
            "DELETE FROM archive_rate_limits WHERE owner_uin=?1",
            params![owner_uin],
        )
        .map_err(|error| format!("清空归档频率记录失败：{error}"))?;
    connection
        .execute(
            "DELETE FROM archive_skips WHERE owner_uin=?1",
            params![owner_uin],
        )
        .map_err(|error| format!("清空异常待重试记录失败：{error}"))?;
    Ok(dynamics as u64)
}

#[tauri::command]
pub async fn delete_all_app_data(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    state: tauri::State<'_, ArchiveState>,
) -> Result<(), String> {
    ensure_archive_idle(&state)?;
    login.clear_session().await;
    let database = database_path(&app)?;
    for path in [
        database.clone(),
        PathBuf::from(format!("{}-wal", database.display())),
        PathBuf::from(format!("{}-shm", database.display())),
    ] {
        if path.exists() {
            fs::remove_file(&path).map_err(|error| format!("删除应用数据库失败：{error}"))?;
        }
    }
    let videos = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("无法获取缓存目录：{error}"))?
        .join("videos");
    if videos.exists() {
        fs::remove_dir_all(videos).map_err(|error| format!("删除视频缓存失败：{error}"))?;
    }
    let images = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取图片归档目录：{error}"))?
        .join("images");
    if images.exists() {
        fs::remove_dir_all(images).map_err(|error| format!("删除图片归档失败：{error}"))?;
    }
    if let Ok(mut progress) = state.progress.lock() {
        *progress = ArchiveProgress::default();
    }
    Ok(())
}

#[tauri::command]
pub async fn list_interactors(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<Vec<Interactor>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare(
            "SELECT actor_uin, COALESCE(MAX(NULLIF(actor_name,'')),actor_uin),
                    COUNT(*),
                    SUM(CASE WHEN event_type=217 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN event_type IN (2,311) THEN 1 ELSE 0 END),
                    MAX(event_time)
             FROM archive_feeds
             WHERE owner_uin=?1 AND actor_uin IS NOT NULL AND actor_uin<>'' AND actor_uin<>?1
               AND event_type IN (2,217,311)
             GROUP BY actor_uin
             ORDER BY COUNT(*) DESC",
        )
        .map_err(|error| format!("准备联系人查询失败：{error}"))?;
    let rows = statement
        .query_map(params![owner_uin], |row| {
            Ok(Interactor {
                uin: row.get(0)?,
                nickname: row.get(1)?,
                total: row.get::<_, i64>(2)?.max(0) as u64,
                likes: row.get::<_, i64>(3)?.max(0) as u64,
                comments: row.get::<_, i64>(4)?.max(0) as u64,
                last_at: row.get(5)?,
            })
        })
        .map_err(|error| format!("查询联系人失败：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取联系人失败：{error}"))
}

// ---------- 留言板全量同步 ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestbookSyncPage {
    fetched: u64,
    saved: u64,
    total: u64,
}

struct GuestbookReply {
    uin: Option<String>,
    nickname: Option<String>,
    reply_to: Option<String>,
    content: String,
    time: i64,
}

pub(crate) struct GuestbookEntry {
    key: String,
    author_uin: Option<String>,
    author_name: Option<String>,
    content: String,
    time: i64,
    replies: Vec<GuestbookReply>,
    raw: Value,
}

/// 兼容多代留言板接口字段：条目主键、作者、正文、时间、回复列表的命名各不相同。
pub(crate) fn parse_guestbook_page(value: &Value) -> (u64, Vec<GuestbookEntry>) {
    let empty = Value::Null;
    let data = value.get("data").unwrap_or(&empty);
    let total = crate::albums::time_field(data, &["total", "commenttotal"])
        .unwrap_or(0)
        .max(0) as u64;
    let list = data
        .get("commentList")
        .or_else(|| data.get("commentlist"))
        .or_else(|| data.get("list"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entries = list
        .iter()
        .filter_map(|entry| {
            let content = entry
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            let author_uin = crate::albums::text_field(entry, &["uin", "posterUin", "posteruin"]);
            let author_name = entry
                .get("nickname")
                .or_else(|| entry.get("posterName"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned);
            let time =
                crate::albums::time_field(entry, &["time", "pubtime", "createTime"]).unwrap_or(0);
            let key = crate::albums::text_field(entry, &["id", "poppubid", "commentid", "giftid"])
                .unwrap_or_else(|| format!("hash-{:016x}", stable_feed_hash(entry)));
            let replies: Vec<GuestbookReply> = entry
                .get("replyList")
                .or_else(|| entry.get("replylist"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|reply| {
                    let content = reply.get("content").and_then(Value::as_str)?.trim();
                    (!content.is_empty()).then(|| GuestbookReply {
                        uin: crate::albums::text_field(reply, &["uin", "replyUin"]),
                        nickname: reply
                            .get("nickname")
                            .or_else(|| reply.get("replyName"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned),
                        reply_to: reply
                            .get("replyName")
                            .or_else(|| reply.get("targetName"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned),
                        content: content.to_owned(),
                        time: crate::albums::time_field(reply, &["time", "pubtime"]).unwrap_or(0),
                    })
                })
                .collect();
            (!content.is_empty() || !replies.is_empty()).then(|| GuestbookEntry {
                key,
                author_uin,
                author_name,
                content,
                time,
                replies,
                raw: entry.clone(),
            })
        })
        .collect();
    (total, entries)
}

/// 把一条留言的回复写成 archive_feeds 行（复用评论展示与导出管线）。
fn guestbook_reply_feed_key(
    owner: &str,
    entry_key: &str,
    index: usize,
    reply: &GuestbookReply,
) -> String {
    format!("gb-{owner}-{entry_key}-r{index}-{}", reply.time)
}

fn reply_comments_json(reply: &GuestbookReply) -> String {
    // 回复对象不在 replys 里重复展开（会渲染两遍），改为内容前缀，语义无损。
    let content = match &reply.reply_to {
        Some(target) => format!("回复 @{target}：{}", reply.content),
        None => reply.content.clone(),
    };
    let mut user = serde_json::Map::new();
    if let Some(uin) = &reply.uin {
        user.insert("uin".into(), Value::String(uin.clone()));
    }
    if let Some(nickname) = &reply.nickname {
        user.insert("nickname".into(), Value::String(nickname.clone()));
    }
    // 与动态评论同构：comments_json 内含 main_comment，复用 comment_from_values 解析。
    json!({
        "main_comment": {
            "commentid": format!("gb-{}", reply.time),
            "content": content,
            "date": reply.time,
            "replynum": 0,
            "replys": [],
            "user": Value::Object(user),
        }
    })
    .to_string()
}

fn save_guestbook_entries(
    connection: &Connection,
    owner_uin: &str,
    entries: &[GuestbookEntry],
    timestamp: i64,
) -> Result<u64, String> {
    let mut saved = 0u64;
    for entry in entries {
        let cell_id = format!("gb-{}", entry.key);
        let changed = connection
            .execute(
                "INSERT INTO archive_dynamics
                   (owner_uin,cell_id,published_at,content,author_uin,author_name,category,raw_original_json,archived_at)
                 VALUES (?1,?2,?3,?4,?5,?6,'guestbook',?7,?8)
                 ON CONFLICT(owner_uin,cell_id) DO UPDATE SET
                   content=excluded.content, author_name=excluded.author_name,
                   raw_original_json=excluded.raw_original_json",
                params![
                    owner_uin,
                    cell_id,
                    entry.time,
                    entry.content,
                    entry.author_uin,
                    entry.author_name,
                    entry.raw.to_string(),
                    timestamp
                ],
            )
            .map_err(|error| format!("保存留言失败：{error}"))?;
        saved += changed as u64;
        for (index, reply) in entry.replies.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO archive_feeds
                       (owner_uin,feed_key,cell_id,event_type,event_time,content,actor_uin,actor_name,comments_json,raw_json,archived_at)
                     VALUES (?1,?2,?3,2,?4,?5,?6,?7,?8,'{}',?9)
                     ON CONFLICT(owner_uin,feed_key) DO UPDATE SET
                       content=excluded.content, comments_json=excluded.comments_json",
                    params![
                        owner_uin,
                        guestbook_reply_feed_key(owner_uin, &entry.key, index, reply),
                        cell_id,
                        reply.time,
                        reply.content,
                        reply.uin,
                        reply.nickname,
                        reply_comments_json(reply),
                        timestamp
                    ],
                )
                .map_err(|error| format!("保存留言回复失败：{error}"))?;
        }
    }
    Ok(saved)
}

/// 抓取一页留言板并入库；前端循环调用直至 fetched=0，实现可取消的分页同步。
#[tauri::command]
pub async fn save_guestbook_page(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    start: u32,
    num: u32,
) -> Result<GuestbookSyncPage, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let page = qzone::fetch_guestbook_page(login.clone(), start, num).await?;
    let (total, entries) = parse_guestbook_page(&page);
    let fetched = entries.len() as u64;
    let connection = open_database(&app)?;
    let saved = save_guestbook_entries(&connection, &owner_uin, &entries, now())?;
    Ok(GuestbookSyncPage {
        fetched,
        saved,
        total,
    })
}

// ---------- 官方说说列表同步 ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShuoshuoSyncPage {
    pub fetched: u64,
    pub saved: u64,
    pub total: u64,
}

/// 解析 emotion_cgi_msglist_v6 响应中的 msglist
pub(crate) fn parse_shuoshuo_page(value: &Value) -> (u64, Vec<Value>) {
    let total = value
        .get("total")
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .get("data")
                .and_then(|d| d.get("total"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let list = value
        .get("msglist")
        .or_else(|| value.get("data").and_then(|d| d.get("msglist")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (total, list)
}

fn save_shuoshuo_entries(
    connection: &Connection,
    owner_uin: &str,
    entries: &[Value],
    timestamp: i64,
) -> Result<u64, String> {
    let mut saved = 0u64;
    for item in entries {
        let tid = item.get("tid").and_then(Value::as_str).unwrap_or("");
        if tid.is_empty() {
            continue;
        }
        let cell_id = tid.to_owned();
        let content = item
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let created_time = item
            .get("created_time")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let uin = item
            .get("uin")
            .and_then(|u| {
                if let Some(s) = u.as_str() {
                    Some(s.to_string())
                } else {
                    u.as_i64().map(|n| n.to_string())
                }
            })
            .unwrap_or_else(|| owner_uin.to_string());
        let name = item.get("name").and_then(Value::as_str);

        // 处理 pic 列表
        let pic_array = item.get("pic").and_then(Value::as_array);
        let pictures_json = pic_array.map(|pics| {
            let photo_list: Vec<Value> = pics
                .iter()
                .filter_map(|p| {
                    let url = p
                        .get("url1")
                        .or_else(|| p.get("url2"))
                        .or_else(|| p.get("url3"))?;
                    Some(json!({
                        "photourl": [{
                            "url": url
                        }]
                    }))
                })
                .collect();
            json!({
                "picdata": {
                    "pic": photo_list
                }
            })
            .to_string()
        });

        let changed = connection
            .execute(
                "INSERT INTO archive_dynamics
                   (owner_uin,cell_id,published_at,content,author_uin,author_name,category,pictures_json,raw_original_json,archived_at)
                 VALUES (?1,?2,?3,?4,?5,?6,'self',?7,?8,?9)
                 ON CONFLICT(owner_uin,cell_id) DO UPDATE SET
                   content=excluded.content,
                   published_at=CASE WHEN archive_dynamics.published_at <= 0 THEN excluded.published_at ELSE archive_dynamics.published_at END,
                   pictures_json=COALESCE(archive_dynamics.pictures_json, excluded.pictures_json),
                   raw_original_json=COALESCE(archive_dynamics.raw_original_json, excluded.raw_original_json)",
                params![
                    owner_uin,
                    cell_id,
                    created_time,
                    content,
                    uin,
                    name,
                    pictures_json,
                    item.to_string(),
                    timestamp,
                ],
            )
            .map_err(|error| format!("保存说说失败：{error}"))?;
        saved += changed as u64;
    }
    Ok(saved)
}

/// 抓取一页官方说说并入库
#[tauri::command]
pub async fn save_shuoshuo_page(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    pos: u32,
    num: u32,
) -> Result<ShuoshuoSyncPage, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let page = qzone::fetch_shuoshuo_page(login.clone(), pos, num).await?;
    let (total, entries) = parse_shuoshuo_page(&page);
    let fetched = entries.len() as u64;
    let connection = open_database(&app)?;
    let saved = save_shuoshuo_entries(&connection, &owner_uin, &entries, now())?;
    Ok(ShuoshuoSyncPage {
        fetched,
        saved,
        total,
    })
}

// ---------- 好友列表 ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendContact {
    uin: String,
    nickname: String,
    remark: Option<String>,
}

/// 兼容 itemsList / list / friends 等不同响应形状与字段命名。
pub(crate) fn parse_friend_list(value: &Value) -> Vec<FriendContact> {
    let empty = Value::Null;
    let data = value.get("data").unwrap_or(&empty);
    let list = data
        .get("itemsList")
        .or_else(|| data.get("list"))
        .or_else(|| data.get("friends"))
        .or_else(|| data.get("friendList"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter_map(|item| {
            let uin = crate::albums::text_field(item, &["uin", "id"])?;
            if uin.is_empty() || uin == "0" {
                return None;
            }
            let nickname = crate::albums::text_field(item, &["nickname", "nick", "name"])
                .unwrap_or_else(|| uin.clone());
            Some(FriendContact {
                uin,
                nickname,
                remark: crate::albums::text_field(item, &["remark", "remarkname"])
                    .filter(|remark| !remark.is_empty()),
            })
        })
        .collect()
}

#[tauri::command]
pub async fn sync_friend_list(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<Vec<FriendContact>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let response = qzone::fetch_friend_list(login.clone()).await?;
    let friends = parse_friend_list(&response);
    if friends.is_empty() {
        return Err("好友列表为空或接口未返回数据，请稍后重试".into());
    }
    let connection = open_database(&app)?;
    let timestamp = now();
    for friend in &friends {
        connection
            .execute(
                "INSERT INTO friend_contacts (owner_uin,uin,nickname,remark,archived_at)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(owner_uin,uin) DO UPDATE SET
                   nickname=excluded.nickname, remark=excluded.remark, archived_at=excluded.archived_at",
                params![owner_uin, friend.uin, friend.nickname, friend.remark, timestamp],
            )
            .map_err(|error| format!("保存好友失败：{error}"))?;
    }
    Ok(friends)
}

#[tauri::command]
pub async fn list_friend_contacts(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<Vec<FriendContact>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let mut statement = connection
        .prepare("SELECT uin,COALESCE(nickname,uin),remark FROM friend_contacts WHERE owner_uin=?1 ORDER BY uin ASC")
        .map_err(|error| format!("读取好友列表失败：{error}"))?;
    let rows = statement
        .query_map(params![owner_uin], |row| {
            Ok(FriendContact {
                uin: row.get(0)?,
                nickname: row.get(1)?,
                remark: row.get(2)?,
            })
        })
        .map_err(|error| format!("读取好友列表失败：{error}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("解析好友列表失败：{error}"))
}

// ---------- JSON / Markdown 导出 ----------

/// 把 unix 秒格式化为本地时区的 "YYYY-MM-DD HH:mm"。
/// 本地时区偏移经 chrono 获取并缓存（HTML 导出走浏览器 toLocaleString，这里需要服务端等价实现）。
pub(crate) fn format_unix_time(seconds: i64) -> String {
    use chrono::Local;
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let offset = *OFFSET.get_or_init(|| Local::now().offset().local_minus_utc() as i64);
    // Howard Hinnant 的 civil_from_days 算法，避免依赖 chrono 的完整格式化栈。
    let seconds = seconds + offset;
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60
    )
}

fn markdown_for_items(items: &[ArchiveItem], category_name: &str, owner_uin: &str) -> String {
    let mut document = String::new();
    document.push_str(&format!(
        "# QQ空间归档 · {category_name}\n\n账号 {owner} · 共 {count} 条\n\n---\n\n",
        category_name = category_name,
        owner = html_escape(owner_uin),
        count = items.len(),
    ));
    for item in items {
        let author = item
            .author_name
            .as_deref()
            .or(item.author_uin.as_deref())
            .unwrap_or("QQ 用户");
        document.push_str(&format!("## {author}\n\n",));
        if let Some(uin) = &item.author_uin {
            document.push_str(&format!("QQ {uin} · "));
        }
        document.push_str(&format_unix_time(item.published_at));
        document.push_str("\n\n");
        if let Some(content) = item
            .content
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            document.push_str(content.trim_start_matches(['：', ':']).trim());
            document.push_str("\n\n");
        }
        for (index, url) in item.picture_urls.iter().enumerate() {
            document.push_str(&format!("![图片 {}]({})\n", index + 1, url));
        }
        if let Some(video) = &item.video_url {
            document.push_str(&format!("[▶ 视频]({video})\n"));
        }
        if !item.likes.is_empty() {
            let names = item
                .likes
                .iter()
                .take(10)
                .map(|like| {
                    like.nickname
                        .as_deref()
                        .or(like.uin.as_deref())
                        .unwrap_or("QQ用户")
                })
                .collect::<Vec<_>>()
                .join("、");
            document.push_str(&format!(
                "\n♥ {}{}\n",
                names,
                if item.likes.len() > 10 {
                    format!(" 等 {} 人", item.like_count)
                } else {
                    String::new()
                }
            ));
        }
        for comment in &item.comments {
            let name = comment.nickname.as_deref().unwrap_or("QQ 用户");
            document.push_str(&format!(
                "\n- **{name}**（{}）：{}",
                format_unix_time(comment.created_at),
                comment.content.replace('\n', "\n  ")
            ));
            for reply in &comment.replies {
                let reply_name = reply.nickname.as_deref().unwrap_or("QQ 用户");
                match reply.reply_to_nickname.as_deref() {
                    Some(target) => document.push_str(&format!(
                        "\n  - **{reply_name}** 回复 **{target}**：{}",
                        reply.content.replace('\n', "\n    ")
                    )),
                    None => document.push_str(&format!(
                        "\n  - **{reply_name}**：{}",
                        reply.content.replace('\n', "\n    ")
                    )),
                }
            }
        }
        document.push_str("\n\n---\n\n");
    }
    document
}

#[tauri::command]
pub async fn export_archived_json(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    category: String,
    ids: Option<Vec<i64>>,
) -> Result<String, String> {
    validate_category(&category)?;
    let owner_uin = login.qzone_auth().await?.uin;
    let selected = ids.map(|values| values.into_iter().collect::<HashSet<_>>());
    let connection = open_database(&app)?;
    let items = archive_items_for_export(&connection, &owner_uin, &category, selected.as_ref())?;
    if items.is_empty() {
        return Err("当前分类没有可以导出的归档".into());
    }
    let payload = json!({
        "owner": owner_uin,
        "category": category,
        "exportedAt": now(),
        "count": items.len(),
        "items": items,
    });
    serde_json::to_string_pretty(&payload).map_err(|error| format!("序列化 JSON 失败：{error}"))
}

#[tauri::command]
pub async fn export_archived_markdown(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    category: String,
    ids: Option<Vec<i64>>,
) -> Result<String, String> {
    validate_category(&category)?;
    let owner_uin = login.qzone_auth().await?.uin;
    let selected = ids.map(|values| values.into_iter().collect::<HashSet<_>>());
    let connection = open_database(&app)?;
    let items = archive_items_for_export(&connection, &owner_uin, &category, selected.as_ref())?;
    if items.is_empty() {
        return Err("当前分类没有可以导出的归档".into());
    }
    let category_name = match category.as_str() {
        "self" => "本人动态",
        "other" => "其他动态",
        _ => "留言",
    };
    Ok(markdown_for_items(&items, category_name, &owner_uin))
}

#[cfg(test)]
mod tests {
    use super::{
        advance_feed_cursor, archive_page_delay_ms, checkpoint_is_stale, parse_feed,
        parse_feed_cursor, serialize_query_pairs, skip_probe_offsets, ArchiveCheckpoint,
        FeedCursorDetails,
    };
    use super::{
        comment_from_values, format_unix_time, health_message, merge_comments, parse_friend_list,
        parse_guestbook_page, reply_comments_json, GuestbookReply,
    };
    use serde_json::json;

    #[test]
    fn parses_guestbook_entry_and_replies() {
        let page = json!({"code":0,"data":{"total":3,"commentList":[
            {"id":"1001","uin":12345,"nickname":"老同学","content":"好久不见！","time":1600000000,
             "replyList":[
                {"uin":67890,"nickname":"主人","content":"是啊","time":1600000300},
                {"uin":12345,"nickname":"老同学","content":"改天聚聚","time":1600000600,"replyName":"主人"}
             ]}
        ]}});
        let (total, entries) = parse_guestbook_page(&page);
        assert_eq!(total, 3);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.key, "1001");
        assert_eq!(entry.author_uin.as_deref(), Some("12345"));
        assert_eq!(entry.author_name.as_deref(), Some("老同学"));
        assert_eq!(entry.content, "好久不见！");
        assert_eq!(entry.time, 1600000000);
        assert_eq!(entry.replies.len(), 2);
        assert_eq!(entry.replies[1].reply_to.as_deref(), Some("主人"));
    }

    #[test]
    fn guestbook_entry_without_id_falls_back_to_stable_hash() {
        let page = json!({"code":0,"data":{"commentList":[
            {"uin":1,"nickname":"a","content":"hello","time":1},
            {"uin":1,"nickname":"a","content":"hello","time":1}
        ]}});
        let (_, entries) = parse_guestbook_page(&page);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].key, entries[1].key,
            "相同内容应得到相同兜底键以便去重"
        );
    }

    #[test]
    fn guestbook_reply_json_carries_reply_target_in_prefix() {
        let reply = GuestbookReply {
            uin: Some("1".into()),
            nickname: Some("甲".into()),
            reply_to: Some("乙".into()),
            content: "好的".into(),
            time: 123,
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&reply_comments_json(&reply))
            .expect("合成 JSON 应可解析");
        assert_eq!(parsed["main_comment"]["user"]["nickname"], "甲");
        let content = parsed["main_comment"]["content"].as_str().unwrap();
        assert!(content.contains("回复 @乙") && content.contains("好的"));
        assert!(parsed["main_comment"]["replys"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parses_friend_list_shapes() {
        let h5 = json!({"code":0,"data":{"itemsList":[
            {"uin":10001,"nickname":"张三","remark":"同学"},
            {"uin":0,"nickname":"无效"},
            {"uin":10002,"nickname":"李四"}
        ]}});
        let friends = parse_friend_list(&h5);
        assert_eq!(friends.len(), 2);
        assert_eq!(friends[0].uin, "10001");
        assert_eq!(friends[0].remark.as_deref(), Some("同学"));
        let legacy = json!({"code":0,"data":{"list":[{"uin":"20002","nick":"王五"}]}});
        let friends = parse_friend_list(&legacy);
        assert_eq!(friends.len(), 1);
        assert_eq!(friends[0].nickname, "王五");
    }

    #[test]
    fn formats_unix_time_as_civil_date() {
        // 与 chrono 的本地时区结果交叉验证，避免手写历法算法漂移。
        use chrono::{Local, TimeZone};
        for seconds in [0_i64, 1_600_000_000, 951_782_400, 4_102_444_800] {
            let expected = Local
                .timestamp_opt(seconds, 0)
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string();
            assert_eq!(format_unix_time(seconds), expected, "seconds={seconds}");
        }
    }

    #[test]
    fn parses_like_event_sample_shape() {
        let feed = json!({"comm":{"feedskey":"217_3_key","subid":217,"time":1752553379},
          "original":{"cell_id":{"cellid":"mood1"},"cell_summary":{"summary":"：纪念"},
          "cell_userinfo":{"user":{"uin":"1","nickname":"主人"}},"cell_video":{"videoid":"v1"}},
          "title":{"title":"赞了我"},"userinfo":{"user":{"uin":"2","nickname":"访客"}}});
        let parsed = parse_feed(&feed).unwrap();
        assert_eq!(parsed.feed_key, "217_3_key");
        assert_eq!(parsed.event_type, 217);
        assert!(parsed.video_json.is_some());
    }

    #[test]
    fn parses_comment_and_picture_sample_shape() {
        let feed = json!({"comm":{"feedskey":"311_2_key","subid":2,"time":1751637966},
          "original":{"cell_id":{"cellid":"mood2"},"cell_summary":{"summary":"：哼哧哼哧"},
          "cell_pic":{"picdata":{"pic":[{},{}]}},"cell_comment":{"main_comment":{"content":"评论"}}},
          "summary":{"summary":"又幸福上了"},"userinfo":{"user":{"uin":"3","nickname":"评论者"}}});
        let parsed = parse_feed(&feed).unwrap();
        assert_eq!(parsed.event_type, 2);
        assert_eq!(parsed.picture_count, 2);
        assert!(parsed.comments_json.is_some());
    }

    #[test]
    fn nests_feed_level_reply_under_its_parent_comment() {
        let comments = json!({
            "main_comment": {
                "content": "给我我好想要",
                "date": 1785795539_i64,
                "replynum": 1,
                "replys": null,
                "user": { "uin": "718038005", "nickname": "此刻春和景明_" }
            }
        });

        let comment = comment_from_values(
            Some(comments.to_string()),
            Some("1027704977".into()),
            Some("轻鹄".into()),
            Some("[em]e10324[/em]".into()),
            1785807754,
        );

        assert_eq!(comment.content, "给我我好想要");
        assert_eq!(comment.replies.len(), 1);
        assert_eq!(comment.replies[0].nickname.as_deref(), Some("轻鹄"));
        assert_eq!(comment.replies[0].content, "[em]e10324[/em]");
        assert_eq!(comment.replies[0].created_at, 1785807754);
    }

    #[test]
    fn does_not_turn_a_regular_comment_into_its_own_reply() {
        let comments = json!({
            "main_comment": {
                "content": "入才",
                "date": 1743068483_i64,
                "replynum": 0,
                "replys": null,
                "user": { "uin": "1027704977", "nickname": "轻鹄" }
            }
        });

        let comment = comment_from_values(
            Some(comments.to_string()),
            Some("1027704977".into()),
            Some("轻鹄".into()),
            Some("入才".into()),
            1743068483,
        );

        assert!(comment.replies.is_empty());
    }

    #[test]
    fn merges_multi_round_replies_and_preserves_each_target() {
        let first = json!({
            "main_comment": {
                "commentid": "parent-1",
                "content": "父评论",
                "date": 100_i64,
                "replynum": 1,
                "replys": [{
                    "content": "第一轮",
                    "date": 110_i64,
                    "user": { "uin": "2", "nickname": "乙" },
                    "replyuser": { "uin": "1", "nickname": "甲" }
                }],
                "user": { "uin": "1", "nickname": "甲" }
            }
        });
        let second = json!({
            "main_comment": {
                "commentid": "parent-1",
                "content": "父评论",
                "date": 100_i64,
                "replynum": 1,
                "replys": [{
                    "content": "第二轮",
                    "date": 120_i64,
                    "user": { "uin": "1", "nickname": "甲" },
                    "replyuser": { "uin": "2", "nickname": "乙" }
                }],
                "user": { "uin": "1", "nickname": "甲" }
            }
        });

        let comments = merge_comments([
            comment_from_values(Some(first.to_string()), None, None, None, 0),
            comment_from_values(Some(second.to_string()), None, None, None, 0),
        ]);

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].replies.len(), 2);
        assert_eq!(comments[0].replies[0].nickname.as_deref(), Some("乙"));
        assert_eq!(
            comments[0].replies[0].reply_to_nickname.as_deref(),
            Some("甲")
        );
        assert_eq!(comments[0].replies[1].nickname.as_deref(), Some("甲"));
        assert_eq!(
            comments[0].replies[1].reply_to_nickname.as_deref(),
            Some("乙")
        );
        assert_eq!(comments[0].replies[1].created_at, 120);
    }

    #[test]
    fn includes_owner_reply_stored_in_comments_array() {
        let comments = json!({
            "comments": [{
                "commentid": "1",
                "content": "嘎嘎咕咕",
                "date": 1786197473_i64,
                "user": { "uin": "1027704977", "nickname": "轻鹄" },
                "replys": [{
                    "replyid": "1",
                    "content": "咕咕咕咕嘎嘎",
                    "date": 1786197525_i64,
                    "user": { "uin": "718038005", "nickname": "此刻春和景明_" },
                    "target": { "uin": "1027704977", "nickname": "轻鹄" }
                }]
            }],
            "main_comment": {
                "commentid": "1",
                "content": "嘎嘎咕咕",
                "date": 1786197473_i64,
                "replynum": 1,
                "replys": null,
                "user": { "uin": "1027704977", "nickname": "轻鹄" }
            }
        });

        let comment = comment_from_values(
            Some(comments.to_string()),
            Some("1027704977".into()),
            Some("轻鹄".into()),
            Some("嘎嘎咕咕".into()),
            1786197473,
        );

        assert_eq!(comment.content, "嘎嘎咕咕");
        assert_eq!(comment.replies.len(), 1);
        assert_eq!(
            comment.replies[0].nickname.as_deref(),
            Some("此刻春和景明_")
        );
        assert_eq!(comment.replies[0].content, "咕咕咕咕嘎嘎");
        assert_eq!(
            comment.replies[0].reply_to_nickname.as_deref(),
            Some("轻鹄")
        );
        assert_eq!(comment.replies[0].created_at, 1786197525);
    }

    #[test]
    fn attaches_parent_author_follow_up_to_latest_child_reply() {
        let comments = json!({
            "comments": [{
                "commentid": "1",
                "content": "嘎嘎咕咕",
                "date": 1786197473_i64,
                "user": { "uin": "1027704977", "nickname": "轻鹄" },
                "replys": [
                    {
                        "replyid": "2",
                        "content": "咕咕嘎嘎咕咕嘎嘎",
                        "date": 1786199046_i64,
                        "user": { "uin": "718038005", "nickname": "此刻春和景明_" },
                        "target": { "uin": "1027704977", "nickname": "轻鹄" }
                    },
                    {
                        "replyid": "3",
                        "content": "凑凑凑凑凑企鹅",
                        "date": 1786199059_i64,
                        "user": { "uin": "718038005", "nickname": "此刻春和景明_" },
                        "target": { "uin": "1027704977", "nickname": "轻鹄" }
                    }
                ]
            }],
            "main_comment": {
                "commentid": "1",
                "content": "嘎嘎咕咕",
                "date": 1786197473_i64,
                "replynum": 4,
                "replys": null,
                "user": { "uin": "1027704977", "nickname": "轻鹄" }
            }
        });

        let comment = comment_from_values(
            Some(comments.to_string()),
            Some("1027704977".into()),
            Some("轻鹄".into()),
            Some("人才咕嘎咕嘎".into()),
            1786199104,
        );

        assert_eq!(comment.replies.len(), 3);
        let follow_up = &comment.replies[2];
        assert_eq!(follow_up.nickname.as_deref(), Some("轻鹄"));
        assert_eq!(follow_up.content, "人才咕嘎咕嘎");
        assert_eq!(
            follow_up.reply_to_nickname.as_deref(),
            Some("此刻春和景明_")
        );
        assert_eq!(follow_up.created_at, 1786199104);
    }

    #[test]
    fn creates_stable_key_for_feed_without_server_identifiers() {
        let feed = json!({
          "comm":{"subid":999,"time":1751637966},
          "summary":{"summary":"一种没有 feedskey 和 cell_id 的特殊互动"},
          "userinfo":{"user":{"uin":"3","nickname":"互动用户"}}
        });

        let first = parse_feed(&feed).expect("特殊互动不应中断整页归档");
        let second = parse_feed(&feed).expect("同一互动应当能重复解析");

        assert!(first.feed_key.starts_with("fallback:999:1751637966:3:"));
        assert_eq!(first.feed_key, second.feed_key);
    }

    #[test]
    fn expires_old_resume_cursor_without_discarding_archive_rows() {
        let checkpoint = ArchiveCheckpoint {
            cursor: "temporary-cursor".into(),
            pages: 78,
            fetched: 706,
            saved: 706,
            updated_at: 1_000,
        };

        // 限流暂停（最长 10 分钟）不应导致续传位置被判为过期
        assert!(!checkpoint_is_stale(&checkpoint, 1_599));
        // 超过 15 分钟的游标才视为陈旧
        assert!(!checkpoint_is_stale(&checkpoint, 1_899));
        assert!(checkpoint_is_stale(&checkpoint, 1_900));
    }

    #[test]
    fn reports_pending_skips_without_marking_database_unhealthy() {
        assert_eq!(
            health_message("ok", 2),
            ("数据库正常，仍有异常位置可在归档任务中重试。", true)
        );
    }

    #[test]
    fn reports_integrity_failure_as_unhealthy() {
        assert_eq!(
            health_message("not ok", 0),
            (
                "数据库完整性检查未通过，请先备份数据并停止继续归档。",
                false
            )
        );
    }

    #[test]
    fn configured_interval_is_never_shortened_by_jitter() {
        let delay = archive_page_delay_ms(3_000);
        assert!((3_000..=3_750).contains(&delay));
    }

    #[test]
    fn advances_nested_qzone_cursor_without_changing_its_time_boundary() {
        let cursor = "att=back%5Fserver%5Finfo%3Doffset%253D1168%2526total%253D4%2526basetime%253D1495974154%2526feedsource%253D1&lastrefreshtime=1785906139&lastseparatortime=0&loadcount=77&refresh_id=1785906139&tl=1495974154";

        assert_eq!(
            parse_feed_cursor(cursor).unwrap(),
            FeedCursorDetails {
                offset: 1168,
                base_time: 1495974154,
                load_count: 77,
            }
        );
        let advanced = advance_feed_cursor(cursor, 2).unwrap();
        assert_eq!(
            parse_feed_cursor(&advanced).unwrap(),
            FeedCursorDetails {
                offset: 1170,
                base_time: 1495974154,
                load_count: 78,
            }
        );
    }

    #[test]
    fn accepts_loadcount_inside_att_and_preserves_that_shape() {
        let backend = serialize_query_pairs(&[
            ("offset".into(), "1168".into()),
            ("basetime".into(), "1495974154".into()),
        ]);
        let attach = serialize_query_pairs(&[
            ("back_server_info".into(), backend),
            ("loadcount".into(), "0".into()),
        ]);
        let cursor =
            serialize_query_pairs(&[("att".into(), attach), ("tl".into(), "1495974154".into())]);

        let advanced = advance_feed_cursor(&cursor, 1).unwrap();
        assert_eq!(
            parse_feed_cursor(&advanced).unwrap(),
            FeedCursorDetails {
                offset: 1169,
                base_time: 1495974154,
                load_count: 1,
            }
        );
        let outer = super::parse_query_pairs(&advanced);
        assert!(super::pair_value(&outer, "loadcount").is_none());
        let attach = super::parse_query_pairs(super::pair_value(&outer, "att").unwrap());
        assert_eq!(super::pair_value(&attach, "loadcount"), Some("1"));
    }

    #[test]
    fn defaults_missing_loadcount_and_adds_it_inside_att() {
        let backend = serialize_query_pairs(&[
            ("offset".into(), "1168".into()),
            ("basetime".into(), "1495974154".into()),
        ]);
        let attach = serialize_query_pairs(&[("back_server_info".into(), backend)]);
        let cursor = serialize_query_pairs(&[("att".into(), attach)]);

        assert_eq!(parse_feed_cursor(&cursor).unwrap().load_count, 0);
        let advanced = advance_feed_cursor(&cursor, 1).unwrap();
        assert_eq!(parse_feed_cursor(&advanced).unwrap().load_count, 1);
    }

    #[test]
    fn probes_large_skip_ranges_exponentially() {
        assert_eq!(
            skip_probe_offsets(1),
            vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096]
        );
        assert_eq!(
            skip_probe_offsets(20),
            vec![20, 32, 64, 128, 256, 512, 1024, 2048, 4096]
        );
    }
}
