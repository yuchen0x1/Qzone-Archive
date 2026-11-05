use std::{
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

use crate::{
    archive::{
        archived_image_dir, download_qzone_image, existing_archived_image, open_database,
        persist_downloaded_image_with_time, reserve_archive_page, set_file_timestamp,
        stable_feed_hash, ArchiveState,
    },
    qlogin::QLoginState,
    qzone,
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumProgress {
    status: &'static str,
    message: String,
    albums_total: u32,
    albums_done: u32,
    photos_total: u64,
    photos_done: u64,
    photos_failed: u64,
    retry_at: Option<i64>,
}

impl Default for AlbumProgress {
    fn default() -> Self {
        Self {
            status: "idle",
            message: "尚未开始相册备份".into(),
            albums_total: 0,
            albums_done: 0,
            photos_total: 0,
            photos_done: 0,
            photos_failed: 0,
            retry_at: None,
        }
    }
}

pub struct AlbumState {
    progress: Mutex<AlbumProgress>,
    cancel: AtomicBool,
}

impl AlbumState {
    pub fn new() -> Self {
        Self {
            progress: Mutex::new(AlbumProgress::default()),
            cancel: AtomicBool::new(false),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    album_id: String,
    name: String,
    description: Option<String>,
    cover_url: Option<String>,
    photo_total: u64,
    create_time: Option<i64>,
    modify_time: Option<i64>,
    /// 该相册在本地是否已有备份数据
    local: bool,
    backed_count: u64,
    failed_count: u64,
    /// 本地备份状态：pending | running | completed | partial | failed
    local_status: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPhoto {
    id: i64,
    album_id: String,
    pic_key: String,
    name: Option<String>,
    description: Option<String>,
    upload_time: Option<i64>,
    preview_url: Option<String>,
    url: Option<String>,
    local_path: Option<String>,
    file_size: i64,
    status: String,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPhotoPage {
    items: Vec<AlbumPhoto>,
    total: u64,
}

pub(crate) struct RemotePhoto {
    pic_key: String,
    name: Option<String>,
    description: Option<String>,
    upload_time: Option<i64>,
    preview_url: Option<String>,
    urls: Vec<String>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn set_album_progress(state: &AlbumState, update: impl FnOnce(&mut AlbumProgress)) {
    if let Ok(mut progress) = state.progress.lock() {
        update(&mut progress);
    }
}

pub(crate) fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let field = value.get(*key)?;
        match field {
            Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    })
}

pub(crate) fn time_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let field = value.get(*key)?;
        if let Some(number) = field.as_i64() {
            return Some(number);
        }
        field.as_str()?.trim().parse::<i64>().ok()
    })
}

/// QQ 接口里的地址可能是字符串，也可能是 {url: "..."} 形式的对象。
/// 只接受 http(s) 地址：旧接口会把照片 ID 之类的非地址字符串放在同名字段里。
fn url_from_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    let text = if let Some(text) = value.as_str() {
        text.trim()
    } else {
        value.get("url")?.as_str()?.trim()
    };
    (!text.is_empty() && text.starts_with("http")).then(|| text.to_owned())
}

fn cover_url_field(value: &Value) -> Option<String> {
    for key in ["coverUrl", "coverurl", "cover"] {
        let Some(cover) = value.get(key) else {
            continue;
        };
        if let Some(url) = url_from_value(Some(cover)) {
            return Some(url);
        }
        for inner in ["pre", "lloc", "thumb"] {
            if let Some(url) = url_from_value(cover.get(inner)) {
                return Some(url);
            }
        }
    }
    None
}

/// 照片地址候选：原图（bloc/lloc）优先，预览图兜底，保持去重后的顺序。
pub(crate) fn collect_photo_url_candidates(value: &Value) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut push = |candidate: Option<String>| {
        if let Some(url) = candidate {
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
    };
    push(url_from_value(value.get("bloc")));
    push(url_from_value(value.get("lloc")));
    if let Some(pic_url) = value.get("picUrl").or_else(|| value.get("picurl")) {
        push(url_from_value(pic_url.get("url")));
        push(url_from_value(pic_url.get("lloc")));
        push(url_from_value(pic_url.get("pre")));
    }
    push(url_from_value(value.get("url")));
    push(url_from_value(value.get("pre")));
    if let Some(thumb) = value.get("thumb") {
        push(url_from_value(thumb.get("url")));
        push(url_from_value(Some(thumb)));
    }
    push(url_from_value(value.get("sloc")));
    urls
}

fn preview_url_field(value: &Value, urls: &[String]) -> Option<String> {
    if let Some(pic_url) = value.get("picUrl").or_else(|| value.get("picurl")) {
        if let Some(url) = url_from_value(pic_url.get("pre")) {
            return Some(url);
        }
    }
    if let Some(url) = url_from_value(value.get("pre")) {
        return Some(url);
    }
    if let Some(thumb) = value.get("thumb") {
        if let Some(url) = url_from_value(thumb.get("url")).or_else(|| url_from_value(Some(thumb)))
        {
            return Some(url);
        }
    }
    urls.last().cloned()
}

/// 不同版本相册接口的字段名不同（H5 v3 与旧版 web 接口），这里做容错归一。
pub(crate) fn parse_remote_album(value: &Value) -> Option<AlbumSummary> {
    let album_id = text_field(value, &["id", "albumId", "albumid", "topicId"])?;
    if album_id.is_empty() {
        return None;
    }
    let name = text_field(value, &["name", "albumname"]).unwrap_or_else(|| "未命名相册".into());
    Some(AlbumSummary {
        album_id,
        name,
        description: text_field(value, &["desc", "description", "albumdesc"]),
        cover_url: cover_url_field(value),
        photo_total: time_field(value, &["total", "photoCount", "photocount"])
            .unwrap_or(0)
            .max(0) as u64,
        create_time: time_field(value, &["createtime", "createTime", "birthTime"]),
        modify_time: time_field(value, &["modifytime", "modifyTime", "lastuploadtime"]),
        local: false,
        backed_count: 0,
        failed_count: 0,
        local_status: None,
    })
}

pub(crate) fn parse_album_list(value: &Value) -> Vec<AlbumSummary> {
    let list = value
        .get("data")
        .and_then(|data| {
            data.get("albumList")
                .or_else(|| data.get("albumlist"))
                .or_else(|| data.get("list"))
        })
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    list.iter().filter_map(parse_remote_album).collect()
}

pub(crate) fn parse_remote_photo(value: &Value) -> Option<RemotePhoto> {
    let pic_key = text_field(
        value,
        &["picKey", "pickey", "picId", "photoid", "lloc", "key"],
    )?;
    if pic_key.is_empty() {
        return None;
    }
    let urls = collect_photo_url_candidates(value);
    if urls.is_empty() {
        return None;
    }
    Some(RemotePhoto {
        pic_key,
        name: text_field(value, &["name", "picname", "title", "filename"]),
        description: text_field(value, &["desc", "description"]),
        upload_time: time_field(value, &["uploadTime", "upload", "createtime"]),
        preview_url: preview_url_field(value, &urls),
        urls,
    })
}

pub(crate) fn parse_photo_list(value: &Value) -> Vec<RemotePhoto> {
    let list = value
        .get("data")
        .and_then(|data| {
            data.get("photoList")
                .or_else(|| data.get("photolist"))
                .or_else(|| data.get("list"))
        })
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    list.iter().filter_map(parse_remote_photo).collect()
}

/// 把相册 ID / 照片键转成安全的文件名片段；包含非常规字符时追加稳定哈希，
/// 保证同一输入跨次运行得到同一文件名（断点续传依赖这一点）。
pub(crate) fn safe_file_stem(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .take(60)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !sanitized.is_empty() && sanitized == raw {
        return sanitized;
    }
    let prefix = sanitized.trim_matches('_');
    format!("{prefix}-{:016x}", stable_feed_hash(&json!(raw)))
}

fn upsert_albums(
    connection: &Connection,
    owner_uin: &str,
    albums: &[AlbumSummary],
    timestamp: i64,
) -> Result<(), String> {
    for album in albums {
        connection
            .execute(
                "INSERT INTO archive_albums
                   (owner_uin,album_id,name,description,cover_url,photo_total,create_time,modify_time,archived_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)
                 ON CONFLICT(owner_uin,album_id) DO UPDATE SET
                   name=excluded.name, description=excluded.description,
                   cover_url=excluded.cover_url, photo_total=excluded.photo_total,
                   create_time=excluded.create_time, modify_time=excluded.modify_time,
                   updated_at=excluded.updated_at",
                params![
                    owner_uin,
                    album.album_id,
                    album.name,
                    album.description,
                    album.cover_url,
                    album.photo_total as i64,
                    album.create_time,
                    album.modify_time,
                    timestamp
                ],
            )
            .map_err(|error| format!("保存相册信息失败：{error}"))?;
    }
    Ok(())
}

fn upsert_photos(
    connection: &Connection,
    owner_uin: &str,
    album_id: &str,
    photos: &[RemotePhoto],
    timestamp: i64,
) -> Result<(), String> {
    for photo in photos {
        let urls_json = serde_json::to_string(&photo.urls).unwrap_or_else(|_| "[]".to_owned());
        connection
            .execute(
                "INSERT INTO archive_album_photos
                   (owner_uin,album_id,pic_key,name,description,upload_time,preview_url,urls_json,archived_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)
                 ON CONFLICT(owner_uin,album_id,pic_key) DO UPDATE SET
                   name=excluded.name, description=excluded.description,
                   upload_time=excluded.upload_time, preview_url=excluded.preview_url,
                   urls_json=excluded.urls_json, updated_at=excluded.updated_at",
                params![
                    owner_uin,
                    album_id,
                    photo.pic_key,
                    photo.name,
                    photo.description,
                    photo.upload_time,
                    photo.preview_url,
                    urls_json,
                    timestamp
                ],
            )
            .map_err(|error| format!("保存相册照片信息失败：{error}"))?;
    }
    Ok(())
}

fn album_photo_counts(
    connection: &Connection,
    owner_uin: &str,
    album_ids: &[String],
) -> Result<(i64, i64, i64), String> {
    if album_ids.is_empty() {
        return Ok((0, 0, 0));
    }
    let placeholders = album_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN status='done' THEN 1 ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END),0)
         FROM archive_album_photos WHERE owner_uin=? AND album_id IN ({placeholders})"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| format!("统计相册照片进度失败：{error}"))?;
    statement
        .query_row(params_from_iter_with_owner(owner_uin, album_ids), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|error| format!("统计相册照片进度失败：{error}"))
}

fn params_from_iter_with_owner<'a>(
    owner_uin: &'a str,
    album_ids: &'a [String],
) -> impl rusqlite::Params + 'a {
    rusqlite::params_from_iter(
        std::iter::once(owner_uin).chain(album_ids.iter().map(String::as_str)),
    )
}

fn apply_local_state(
    connection: &Connection,
    owner_uin: &str,
    summary: &mut AlbumSummary,
) -> Result<(), String> {
    let (backed, failed, status): (i64, i64, Option<String>) = connection
        .query_row(
            "SELECT COALESCE(SUM(CASE WHEN status='done' THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END),0)
             FROM archive_album_photos WHERE owner_uin=?1 AND album_id=?2",
            params![owner_uin, summary.album_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, None::<String>)),
        )
        .map_err(|error| format!("读取相册本地状态失败：{error}"))?;
    summary.backed_count = backed.max(0) as u64;
    summary.failed_count = failed.max(0) as u64;
    summary.local_status = status;
    summary.local = summary.backed_count > 0;
    Ok(())
}

fn local_album_rows(connection: &Connection, owner_uin: &str) -> Result<Vec<AlbumSummary>, String> {
    let mut statement = connection
        .prepare(
            "SELECT album_id,COALESCE(name,'未命名相册'),description,cover_url,photo_total,
                    create_time,modify_time,status FROM archive_albums
             WHERE owner_uin=?1 ORDER BY modify_time IS NULL, modify_time DESC, id ASC",
        )
        .map_err(|error| format!("读取本地相册失败：{error}"))?;
    let mut summaries = statement
        .query_map(params![owner_uin], |row| {
            Ok(AlbumSummary {
                album_id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                cover_url: row.get(3)?,
                photo_total: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as u64,
                create_time: row.get(5)?,
                modify_time: row.get(6)?,
                local_status: row.get(7)?,
                local: false,
                backed_count: 0,
                failed_count: 0,
            })
        })
        .map_err(|error| format!("读取本地相册失败：{error}"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    for summary in &mut summaries {
        apply_local_state(connection, owner_uin, summary)?;
    }
    Ok(summaries)
}

/// 一张待下载照片的纯数据任务：跨 await 的下载流程不携带数据库连接。
pub(crate) struct AlbumPhotoTask {
    photo_id: i64,
    album_id: String,
    pic_key: String,
    upload_time: Option<i64>,
    urls: Vec<String>,
}

fn load_album_photo_task(
    connection: &Connection,
    owner_uin: &str,
    photo_id: i64,
) -> Result<AlbumPhotoTask, String> {
    connection
        .query_row(
            "SELECT album_id,pic_key,urls_json,upload_time FROM archive_album_photos
             WHERE id=?1 AND owner_uin=?2",
            params![photo_id, owner_uin],
            |row| {
                let urls_json: String = row.get(2)?;
                Ok(AlbumPhotoTask {
                    photo_id,
                    album_id: row.get(0)?,
                    pic_key: row.get(1)?,
                    urls: serde_json::from_str(&urls_json).unwrap_or_default(),
                    upload_time: row.get(3)?,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => "相册照片记录不存在或已删除".into(),
            _ => format!("读取相册照片记录失败：{error}"),
        })
}

/// 下载单张相册照片；幂等：本地已有文件时直接返回。绝不触碰数据库，
/// 因此可以在不阻塞 sqlite 连接的情况下安全等待网络。
async fn process_album_photo(
    app: &tauri::AppHandle,
    login: &QLoginState,
    task: &AlbumPhotoTask,
) -> Result<(PathBuf, u64), String> {
    let auth = login.qzone_auth().await?;
    let image_dir = archived_image_dir(app, &auth.uin)?
        .join("albums")
        .join(safe_file_stem(&task.album_id));
    fs::create_dir_all(&image_dir).map_err(|error| format!("无法创建相册照片目录：{error}"))?;
    let file_stem = safe_file_stem(&task.pic_key);
    if let Some(path) = existing_archived_image(&image_dir, &file_stem) {
        let size = path.metadata().map(|meta| meta.len()).unwrap_or(0);
        if let Some(ts) = task.upload_time {
            set_file_timestamp(&path, ts);
        }
        return Ok((path, size));
    }
    if task.urls.is_empty() {
        return Err("该照片没有保存可用的 QQ 地址".into());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|error| format!("创建图片请求客户端失败：{error}"))?;
    let downloaded = download_qzone_image(&auth, &task.urls, &client).await?;
    let path =
        persist_downloaded_image_with_time(&image_dir, &file_stem, &downloaded, task.upload_time)?;
    let size = downloaded.bytes.len() as u64;
    Ok((path, size))
}

fn mark_album_photo_done(
    connection: &Connection,
    photo_id: i64,
    path: &std::path::Path,
    size: i64,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE archive_album_photos SET status='done', local_path=?2, file_size=?3,
                    error=NULL, updated_at=?4 WHERE id=?1",
            params![photo_id, path.to_string_lossy(), size, now()],
        )
        .map_err(|error| format!("更新相册照片状态失败：{error}"))?;
    Ok(())
}

fn mark_album_photo_failed(
    connection: &Connection,
    photo_id: i64,
    error: &str,
) -> Result<(), String> {
    let summary: String = error.split_whitespace().collect::<Vec<_>>().join(" ");
    let summary = summary.chars().take(200).collect::<String>();
    connection
        .execute(
            "UPDATE archive_album_photos SET status='failed', error=?2, updated_at=?3 WHERE id=?1",
            params![photo_id, summary, now()],
        )
        .map_err(|error| format!("记录相册照片失败原因失败：{error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn start_album_backup(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    albums: tauri::State<'_, AlbumState>,
    archive: tauri::State<'_, ArchiveState>,
    interval_ms: u64,
    album_ids: Option<Vec<String>>,
) -> Result<AlbumProgress, String> {
    if archive.is_feed_archive_running() {
        return Err("动态归档任务正在运行，请等待完成后再备份相册".into());
    }
    {
        let mut progress = albums.progress.lock().map_err(|_| "相册任务状态锁已损坏")?;
        if progress.status == "running" {
            return Err("已有相册备份任务正在运行".into());
        }
        *progress = AlbumProgress {
            status: "running",
            message: "正在获取相册列表…".into(),
            ..Default::default()
        };
    }
    albums.cancel.store(false, Ordering::Relaxed);
    let interval_ms = interval_ms.clamp(2_000, 30_000);
    let result: Result<(), String> = async {
        let owner_uin = login.qzone_auth().await?.uin;
        let timestamp = now();
        let remote = qzone::fetch_album_list_json(&login).await?;
        let mut summaries = parse_album_list(&remote);
        if let Some(selected) = album_ids.as_ref().filter(|ids| !ids.is_empty()) {
            let wanted: std::collections::HashSet<&String> = selected.iter().collect();
            summaries.retain(|album| wanted.contains(&album.album_id));
        }
        let selected_ids: Vec<String> = if album_ids.as_ref().is_some_and(|ids| !ids.is_empty()) {
            album_ids.clone().expect("上方已确认存在且非空")
        } else {
            summaries
                .iter()
                .map(|album| album.album_id.clone())
                .collect()
        };
        {
            let connection = open_database(&app)?;
            upsert_albums(&connection, &owner_uin, &summaries, timestamp)?;
            // 本地已有但远端未返回的相册（例如仅重试本地备份）也补一条记录
            if let Some(selected) = album_ids.as_ref().filter(|ids| !ids.is_empty()) {
                for album_id in selected {
                    connection
                        .execute(
                            "INSERT OR IGNORE INTO archive_albums
                               (owner_uin,album_id,name,status,archived_at,updated_at)
                             VALUES (?1,?2,'未知相册','pending',?3,?3)",
                            params![owner_uin, album_id, timestamp],
                        )
                        .map_err(|error| format!("保存相册信息失败：{error}"))?;
                }
            }
        }
        if selected_ids.is_empty() {
            set_album_progress(&albums, |progress| {
                progress.message = "当前账号没有可备份的相册".into();
            });
            return Ok(());
        }
        let total_albums = selected_ids.len() as u32;
        for (index, album_id) in selected_ids.iter().enumerate() {
            if albums.cancel.load(Ordering::Relaxed) {
                break;
            }
            // sqlite 连接不能跨 await 持有：所有数据库操作都用独立作用域包住
            let display_name: String = {
                let connection = open_database(&app)?;
                let display_name: String = connection
                    .query_row(
                        "SELECT COALESCE(name,'未命名相册') FROM archive_albums
                         WHERE owner_uin=?1 AND album_id=?2",
                        params![owner_uin, album_id],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| album_id.to_string());
                connection
                    .execute(
                        "UPDATE archive_albums SET status='running', updated_at=?3
                         WHERE owner_uin=?1 AND album_id=?2",
                        params![owner_uin, album_id, now()],
                    )
                    .map_err(|error| format!("更新相册状态失败：{error}"))?;
                display_name
            };
            set_album_progress(&albums, |progress| {
                progress.message = format!(
                    "({}/{}) 正在读取相册「{}」的照片列表…",
                    index + 1,
                    total_albums,
                    display_name
                );
            });

            // 2.1 分页列出照片并入库。服务端可能把单页封顶在 100（即使请求更大），
            // 因此不能用“返回条数 < 请求条数”判断结束，而是翻到空页或连续无新照片为止；
            // 以 pic_key 去重防止接口在末页反复返回同一页导致死循环。
            const PAGE_NUM: u32 = 100;
            const MAX_PAGES: u32 = 500;
            let mut page_start: u32 = 0;
            let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut server_total: Option<u64> = None;
            for _page_index in 0..MAX_PAGES {
                if albums.cancel.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(retry_at) = reserve_archive_page(&app, &owner_uin)? {
                    return Err(format!("ALBUM_RATE_LIMIT:{retry_at}"));
                }
                let page = qzone::fetch_photo_page(&login, album_id, page_start, PAGE_NUM).await?;
                if server_total.is_none() {
                    server_total = crate::albums::time_field(
                        page.get("data").unwrap_or(&Value::Null),
                        &["total", "photocount", "photoTotal"],
                    )
                    .map(|total| total.max(0) as u64);
                }
                let photos = parse_photo_list(&page);
                let new_count = photos
                    .iter()
                    .filter(|photo| seen_keys.insert(photo.pic_key.clone()))
                    .count();
                {
                    let connection = open_database(&app)?;
                    upsert_photos(&connection, &owner_uin, album_id, &photos, now())?;
                }
                let reached_total =
                    server_total.is_some_and(|total| seen_keys.len() as u64 >= total);
                if photos.is_empty() || new_count == 0 || reached_total {
                    break;
                }
                page_start = page_start.saturating_add(PAGE_NUM);
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms.max(2_000))).await;
            }

            // 2.2 顺序下载待备份照片；status='done' 的自动跳过 → 断点续传
            let pending_tasks: Vec<AlbumPhotoTask> = {
                let connection = open_database(&app)?;
                let mut statement = connection
                    .prepare(
                        "SELECT id,pic_key,urls_json,upload_time FROM archive_album_photos
                             WHERE owner_uin=?1 AND album_id=?2 AND status='pending'
                             ORDER BY id ASC",
                    )
                    .map_err(|error| format!("读取待备份照片失败：{error}"))?;
                let rows = statement
                    .query_map(params![owner_uin, album_id], |row| {
                        let urls_json: String = row.get(2)?;
                        Ok(AlbumPhotoTask {
                            photo_id: row.get(0)?,
                            album_id: album_id.clone(),
                            pic_key: row.get(1)?,
                            urls: serde_json::from_str(&urls_json).unwrap_or_default(),
                            upload_time: row.get(3)?,
                        })
                    })
                    .map_err(|error| format!("读取待备份照片失败：{error}"))?;
                rows.filter_map(Result::ok).collect()
            };
            for task in pending_tasks {
                if albums.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let outcome = process_album_photo(&app, &login, &task).await;
                {
                    let connection = open_database(&app)?;
                    match &outcome {
                        Ok((path, size)) => {
                            mark_album_photo_done(&connection, task.photo_id, path, *size as i64)?
                        }
                        Err(error) => mark_album_photo_failed(&connection, task.photo_id, error)?,
                    }
                    let (total, done, failed) =
                        album_photo_counts(&connection, &owner_uin, &selected_ids)?;
                    set_album_progress(&albums, |progress| {
                        progress.photos_total = total.max(0) as u64;
                        progress.photos_done = done.max(0) as u64;
                        progress.photos_failed = failed.max(0) as u64;
                        let failure_note = if progress.photos_failed > 0 {
                            format!("，失败 {} 张", progress.photos_failed)
                        } else {
                            String::new()
                        };
                        progress.message = format!(
                            "({}/{}) 正在备份相册「{}」：已完成 {} / {} 张{}",
                            index + 1,
                            total_albums,
                            display_name,
                            progress.photos_done,
                            progress.photos_total,
                            failure_note
                        );
                    });
                }
            }

            // 2.3 汇总本相册结果
            let (album_total, album_done, album_failed) = {
                let connection = open_database(&app)?;
                album_photo_counts(&connection, &owner_uin, std::slice::from_ref(album_id))?
            };
            let album_status = if albums.cancel.load(Ordering::Relaxed)
                && album_done + album_failed < album_total
            {
                "partial"
            } else if album_failed == 0 {
                "completed"
            } else if album_done > 0 {
                "partial"
            } else {
                "failed"
            };
            {
                let connection = open_database(&app)?;
                connection
                    .execute(
                        "UPDATE archive_albums SET status=?3, backed_count=?4, failed_count=?5,
                            updated_at=?6 WHERE owner_uin=?1 AND album_id=?2",
                        params![
                            owner_uin,
                            album_id,
                            album_status,
                            album_done,
                            album_failed,
                            now()
                        ],
                    )
                    .map_err(|error| format!("更新相册备份结果失败：{error}"))?;
            }
            set_album_progress(&albums, |progress| {
                progress.albums_total = total_albums;
                progress.albums_done = index as u32 + 1;
            });
        }
        Ok(())
    }
    .await;
    match &result {
        Ok(()) if albums.cancel.load(Ordering::Relaxed) => set_album_progress(&albums, |p| {
            p.status = "cancelled";
            p.message = "相册备份已取消，再次开始将从当前进度继续".into();
            p.retry_at = None;
        }),
        Ok(()) => set_album_progress(&albums, |p| {
            p.status = "completed";
            p.message = if p.photos_failed > 0 {
                format!(
                    "相册备份完成：{} 个相册，成功 {} 张，失败 {} 张（可对失败相册单独重试）",
                    p.albums_done, p.photos_done, p.photos_failed
                )
            } else {
                format!(
                    "相册备份完成：{} 个相册，共 {} 张照片已保存到本地",
                    p.albums_done, p.photos_done
                )
            };
            p.retry_at = None;
        }),
        Err(error) if error.starts_with("ALBUM_RATE_LIMIT:") => {
            let retry_at = error
                .trim_start_matches("ALBUM_RATE_LIMIT:")
                .parse::<i64>()
                .ok();
            set_album_progress(&albums, |p| {
                p.status = "limited";
                p.retry_at = retry_at;
                p.message = "为防止接口请求过于频繁，每 10 分钟最多请求 300 页。相册备份已安全暂停，倒计时结束即可从当前进度继续。".into();
            });
        }
        Err(error) => set_album_progress(&albums, |p| {
            p.status = "error";
            p.message = format!("相册备份失败：{}", concise_error(error));
            p.retry_at = None;
        }),
    }
    let progress = albums
        .progress
        .lock()
        .map_err(|_| "相册任务状态锁已损坏")?
        .clone();
    if result
        .as_ref()
        .is_err_and(|error| error.starts_with("ALBUM_RATE_LIMIT:"))
    {
        Ok(progress)
    } else {
        result.map(|_| progress)
    }
}

fn concise_error(error: &str) -> String {
    let normalized = error.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let summary = chars.by_ref().take(240).collect::<String>();
    if chars.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}

#[tauri::command]
pub fn get_album_progress(albums: tauri::State<'_, AlbumState>) -> Result<AlbumProgress, String> {
    albums
        .progress
        .lock()
        .map(|value| value.clone())
        .map_err(|_| "相册任务状态锁已损坏".into())
}

#[tauri::command]
pub fn cancel_album_backup(albums: tauri::State<'_, AlbumState>) {
    albums.cancel.store(true, Ordering::Relaxed);
}

/// 远端相册列表（合并本地备份状态）。需要网络与登录会话。
#[tauri::command]
pub async fn list_remote_albums(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<Vec<AlbumSummary>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let remote = qzone::fetch_album_list_json(&login).await?;
    let mut summaries = parse_album_list(&remote);
    let connection = open_database(&app)?;
    upsert_albums(&connection, &owner_uin, &summaries, now())?;
    let mut merged = Vec::with_capacity(summaries.len());
    for mut summary in summaries.drain(..) {
        apply_local_state(&connection, &owner_uin, &mut summary)?;
        merged.push(summary);
    }
    for local in local_album_rows(&connection, &owner_uin)? {
        if !merged
            .iter()
            .any(|summary| summary.album_id == local.album_id)
        {
            merged.push(local);
        }
    }
    Ok(merged)
}

/// 仅本地相册列表（无网络请求），用于离线浏览已备份内容。
#[tauri::command]
pub async fn list_archived_albums(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
) -> Result<Vec<AlbumSummary>, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    local_album_rows(&connection, &owner_uin)
}

#[tauri::command]
pub async fn list_archived_album_photos(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    album_id: String,
    limit: u32,
    offset: u32,
) -> Result<AlbumPhotoPage, String> {
    if album_id.trim().is_empty() {
        return Err("缺少相册 ID".into());
    }
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let total: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM archive_album_photos WHERE owner_uin=?1 AND album_id=?2",
            params![owner_uin, album_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("统计相册照片失败：{error}"))?;
    let mut statement = connection
        .prepare(
            "SELECT id,album_id,pic_key,name,description,upload_time,preview_url,urls_json,
                    local_path,file_size,status,error
             FROM archive_album_photos
             WHERE owner_uin=?1 AND album_id=?2
             ORDER BY upload_time IS NULL, upload_time ASC, id ASC
             LIMIT ?3 OFFSET ?4",
        )
        .map_err(|error| format!("读取相册照片失败：{error}"))?;
    let items = statement
        .query_map(
            params![owner_uin, album_id, limit.clamp(1, 200), offset],
            |row| {
                let urls_json: String = row.get(7)?;
                let urls: Vec<String> = serde_json::from_str(&urls_json).unwrap_or_default();
                Ok(AlbumPhoto {
                    id: row.get(0)?,
                    album_id: row.get(1)?,
                    pic_key: row.get(2)?,
                    name: row.get(3)?,
                    description: row.get(4)?,
                    upload_time: row.get(5)?,
                    preview_url: row.get(6)?,
                    url: urls.first().cloned(),
                    local_path: row.get(8)?,
                    file_size: row.get(9)?,
                    status: row.get(10)?,
                    error: row.get(11)?,
                })
            },
        )
        .map_err(|error| format!("读取相册照片失败：{error}"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    Ok(AlbumPhotoPage {
        items,
        total: total.max(0) as u64,
    })
}

/// 把失败照片重置为待备份，随后可重新开始相册备份。
#[tauri::command]
pub async fn reset_failed_album_photos(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    albums: tauri::State<'_, AlbumState>,
    album_id: String,
) -> Result<u64, String> {
    if albums
        .progress
        .lock()
        .map_err(|_| "相册任务状态锁已损坏")?
        .status
        == "running"
    {
        return Err("相册备份进行中，请先停止再重试".into());
    }
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let changed = connection
        .execute(
            "UPDATE archive_album_photos SET status='pending', error=NULL, updated_at=?3
             WHERE owner_uin=?1 AND album_id=?2 AND status='failed'",
            params![owner_uin, album_id, now()],
        )
        .map_err(|error| format!("重置失败照片失败：{error}"))?;
    connection
        .execute(
            "UPDATE archive_albums SET status='pending', updated_at=?3
             WHERE owner_uin=?1 AND album_id=?2",
            params![owner_uin, album_id, now()],
        )
        .map_err(|error| format!("重置相册状态失败：{error}"))?;
    Ok(changed as u64)
}

/// 删除一个相册的本地备份（数据库记录 + 已下载文件），云端不受影响。
#[tauri::command]
pub async fn delete_archived_album(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    albums: tauri::State<'_, AlbumState>,
    album_id: String,
) -> Result<u64, String> {
    if albums
        .progress
        .lock()
        .map_err(|_| "相册任务状态锁已损坏")?
        .status
        == "running"
    {
        return Err("相册备份进行中，请先停止再删除".into());
    }
    let owner_uin = login.qzone_auth().await?.uin;
    let connection = open_database(&app)?;
    let paths: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT local_path FROM archive_album_photos
                 WHERE owner_uin=?1 AND album_id=?2 AND local_path IS NOT NULL",
            )
            .map_err(|error| format!("读取相册文件列表失败：{error}"))?;
        let rows = statement
            .query_map(params![owner_uin, album_id], |row| row.get(0))
            .map_err(|error| format!("读取相册文件列表失败：{error}"))?;
        rows.filter_map(Result::ok).collect()
    };
    let removed = connection
        .execute(
            "DELETE FROM archive_album_photos WHERE owner_uin=?1 AND album_id=?2",
            params![owner_uin, album_id],
        )
        .map_err(|error| format!("删除相册照片记录失败：{error}"))?;
    connection
        .execute(
            "DELETE FROM archive_albums WHERE owner_uin=?1 AND album_id=?2",
            params![owner_uin, album_id],
        )
        .map_err(|error| format!("删除相册记录失败：{error}"))?;
    for path in &paths {
        let _ = fs::remove_file(path);
    }
    if let Ok(image_dir) = archived_image_dir(&app, &owner_uin) {
        let album_dir = image_dir.join("albums").join(safe_file_stem(&album_id));
        if album_dir.exists() {
            let _ = fs::remove_dir_all(album_dir);
        }
    }
    Ok(removed as u64)
}

/// 按需加载单张相册照片的本地文件（未缓存时自动下载）。
#[tauri::command]
pub async fn load_album_photo(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    photo_id: i64,
) -> Result<String, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let task = {
        let connection = open_database(&app)?;
        load_album_photo_task(&connection, &owner_uin, photo_id)?
    };
    let (path, size) = process_album_photo(&app, &login, &task).await?;
    {
        let connection = open_database(&app)?;
        mark_album_photo_done(&connection, photo_id, &path, size as i64)?;
    }
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        collect_photo_url_candidates, parse_album_list, parse_photo_list, parse_remote_album,
        parse_remote_photo, safe_file_stem,
    };
    use serde_json::json;

    #[test]
    fn parses_h5_v3_album_shape() {
        let value = json!({"code":0,"data":{"albumList":[
            {"id":"album-1","name":"旅行","desc":"2019 年","total":42,
             "createtime":1550000000,"modifytime":1570000000,
             "coverUrl":{"url":"https://a.qpic.cn/cover.jpg"}}
        ]}});
        let albums = parse_album_list(&value);
        assert_eq!(albums.len(), 1);
        let album = &albums[0];
        assert_eq!(album.album_id, "album-1");
        assert_eq!(album.name, "旅行");
        assert_eq!(album.description.as_deref(), Some("2019 年"));
        assert_eq!(album.photo_total, 42);
        assert_eq!(
            album.cover_url.as_deref(),
            Some("https://a.qpic.cn/cover.jpg")
        );
        assert_eq!(album.create_time, Some(1550000000));
    }

    #[test]
    fn parses_legacy_album_shape() {
        let value = json!({"code":0,"data":{"albumList":[
            {"albumid":"album-2","albumname":"旧接口","photocount":7}
        ]}});
        let albums = parse_album_list(&value);
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album_id, "album-2");
        assert_eq!(albums[0].name, "旧接口");
        assert_eq!(albums[0].photo_total, 7);
        assert!(!albums[0].local);
    }

    #[test]
    fn ignores_album_without_id() {
        let value = json!({"code":0,"data":{"albumList":[{"name":"没有 id"}]}});
        assert!(parse_album_list(&value).is_empty());
    }

    #[test]
    fn prefers_original_photo_urls() {
        let value = json!({
            "picKey": "p1",
            "picUrl": {"pre": "https://a.qpic.cn/pre.jpg", "url": "https://a.qpic.cn/full.jpg"},
            "url": "https://a.qpic.cn/top.jpg"
        });
        let urls = collect_photo_url_candidates(&value);
        assert_eq!(
            urls,
            vec![
                "https://a.qpic.cn/full.jpg",
                "https://a.qpic.cn/pre.jpg",
                "https://a.qpic.cn/top.jpg"
            ]
        );
    }

    #[test]
    fn parses_photo_with_nested_url_object() {
        let value = json!({
            "picKey": "p2",
            "name": "IMG_0001.jpg",
            "uploadTime": 1570001234,
            "lloc": {"url": "https://a.qpic.cn/original.jpg"},
            "pre": "https://a.qpic.cn/preview.jpg"
        });
        let photo = parse_photo_list(&json!({"code":0,"data":{"photoList":[value]}}))
            .into_iter()
            .next()
            .expect("照片应可解析");
        assert_eq!(photo.pic_key, "p2");
        assert_eq!(photo.name.as_deref(), Some("IMG_0001.jpg"));
        assert_eq!(photo.upload_time, Some(1570001234));
        assert_eq!(
            photo.urls.first().map(String::as_str),
            Some("https://a.qpic.cn/original.jpg")
        );
        assert_eq!(
            photo.preview_url.as_deref(),
            Some("https://a.qpic.cn/preview.jpg")
        );
    }

    #[test]
    fn parses_legacy_photo_key_from_lloc_string() {
        let value = json!({
            "lloc": "legacy-key",
            "picname": "old.jpg",
            "url": "https://a.qpic.cn/old-full.jpg"
        });
        let photo = parse_remote_photo(&value).expect("旧格式照片应可解析");
        assert_eq!(photo.pic_key, "legacy-key");
        assert_eq!(photo.name.as_deref(), Some("old.jpg"));
        assert_eq!(photo.urls.len(), 1);
    }

    #[test]
    fn rejects_photo_without_url() {
        let value = json!({"picKey": "p3", "name": "no-url.jpg"});
        assert!(parse_remote_photo(&value).is_none());
    }

    #[test]
    fn safe_file_stem_is_deterministic() {
        assert_eq!(safe_file_stem("abc-123"), "abc-123");
        let first = safe_file_stem("weird/../key 值");
        let second = safe_file_stem("weird/../key 值");
        assert_eq!(first, second);
        assert!(!first.contains('/') && !first.contains(' '));
        assert_ne!(safe_file_stem("a/b"), safe_file_stem("a_b"));
    }

    #[test]
    fn parse_remote_album_tolerates_missing_optional_fields() {
        let album = parse_remote_album(&json!({"id":"album-3"})).expect("最小字段应可解析");
        assert_eq!(album.name, "未命名相册");
        assert_eq!(album.photo_total, 0);
        assert!(album.cover_url.is_none());
    }
}
