use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::json;
use tauri::Manager;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use crate::{
    archive::{
        archive_items_for_export_since, archived_image_dir, existing_archived_image, feed_document,
        html_escape, open_database, validate_category, video_cache_path, ArchiveItem,
        ExportMediaResolver, EXPORT_CSS,
    },
    qlogin::QLoginState,
};

const CATEGORIES: [(&str, &str); 3] = [
    ("self", "本人动态"),
    ("other", "其他动态"),
    ("guestbook", "留言"),
];

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    status: &'static str,
    current: u64,
    total: u64,
    message: String,
}

impl Default for ExportProgress {
    fn default() -> Self {
        Self {
            status: "idle",
            current: 0,
            total: 0,
            message: "尚未开始导出".into(),
        }
    }
}

#[derive(Clone)]
pub struct ExportState {
    progress: Arc<Mutex<ExportProgress>>,
    cancel: Arc<AtomicBool>,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            progress: Arc::new(Mutex::new(ExportProgress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ExportState {
    pub fn new() -> Self {
        Self::default()
    }

    fn set_progress(&self, update: impl FnOnce(&mut ExportProgress)) {
        if let Ok(mut progress) = self.progress.lock() {
            update(&mut progress);
        }
    }

    fn snapshot(&self) -> Result<ExportProgress, String> {
        self.progress
            .lock()
            .map(|progress| progress.clone())
            .map_err(|_| "导出状态锁已损坏".into())
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

fn advance_progress(state: &ExportState, amount: u64, message: impl Into<String>) {
    state.set_progress(|progress| {
        progress.current = progress.current.saturating_add(amount).min(progress.total);
        progress.message = message.into();
    });
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPackageStats {
    target_path: String,
    dynamic_count: u64,
    html_files: u32,
    media_files: u64,
    album_count: u32,
    total_bytes: u64,
}

/// 导出离线包：HTML 分类页 + 相册页 + 已缓存的图片/视频 + 结构化 data.json，
/// 全部打包为一个 zip。包内 HTML 使用相对路径引用媒体，可在任何设备离线打开。
#[tauri::command]
pub async fn export_archive_package(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    state: tauri::State<'_, ExportState>,
    target_path: String,
    categories: Option<Vec<String>>,
    include_albums: Option<bool>,
    include_media: Option<bool>,
    incremental: Option<bool>,
) -> Result<ExportPackageStats, String> {
    let owner_uin = login.qzone_auth().await?.uin;
    let categories = categories.unwrap_or_default();
    let mut wanted: Vec<String> = Vec::new();
    if categories.is_empty() {
        wanted = CATEGORIES.iter().map(|(id, _)| (*id).to_owned()).collect();
    } else {
        for category in &categories {
            validate_category(category)?;
            if !wanted.iter().any(|saved| saved == category) {
                wanted.push(category.clone());
            }
        }
    }
    if wanted.is_empty() {
        return Err("请至少选择一个要导出的分类".into());
    }
    if target_path.trim().is_empty() {
        return Err("请先选择导出位置".into());
    }
    {
        let progress = state.snapshot()?;
        if progress.status == "running" {
            return Err("已有导出任务正在运行".into());
        }
    }
    state.cancel.store(false, Ordering::Relaxed);
    state.set_progress(|progress| {
        *progress = ExportProgress {
            status: "running",
            current: 0,
            total: 0,
            message: "正在准备导出…".into(),
        };
    });
    let state = state.inner().clone();
    let include_albums = include_albums.unwrap_or(true);
    let include_media = include_media.unwrap_or(true);
    let incremental = incremental.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        build_package(
            &app,
            &owner_uin,
            target_path.trim(),
            &wanted,
            include_albums,
            include_media,
            incremental,
            &state,
        )
    })
    .await
    .map_err(|error| format!("导出任务异常退出：{error}"))?
}

#[allow(clippy::too_many_arguments)]
fn build_package(
    app: &tauri::AppHandle,
    owner_uin: &str,
    target_path: &str,
    wanted: &[String],
    include_albums: bool,
    include_media: bool,
    incremental: bool,
    state: &ExportState,
) -> Result<ExportPackageStats, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("无法获取缓存目录：{error}"))?
        .join(format!("export-{nonce}"));
    let media_dir = staging.join("media");
    let outcome = (|| {
        fs::create_dir_all(&media_dir).map_err(|error| format!("无法创建导出目录：{error}"))?;
        let export_started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        build_package_contents(
            app,
            owner_uin,
            &staging,
            &media_dir,
            target_path,
            wanted,
            include_albums,
            include_media,
            incremental,
            export_started_at,
            state,
        )
    })();
    let _ = fs::remove_dir_all(&staging);
    match &outcome {
        Ok(stats) => state.set_progress(|progress| {
            progress.status = "completed";
            progress.current = progress.total;
            progress.message = format!("导出完成，共生成 {} 个 HTML 文件", stats.html_files);
        }),
        Err(error) if error == "EXPORT_CANCELLED" => state.set_progress(|progress| {
            progress.status = "cancelled";
            progress.message = "导出已取消".into();
        }),
        Err(error) => state.set_progress(|progress| {
            progress.status = "error";
            progress.message = format!("导出失败：{error}");
        }),
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
fn build_package_contents(
    app: &tauri::AppHandle,
    owner_uin: &str,
    staging: &Path,
    media_dir: &Path,
    target_path: &str,
    wanted: &[String],
    include_albums: bool,
    include_media: bool,
    incremental: bool,
    export_started_at: i64,
    state: &ExportState,
) -> Result<ExportPackageStats, String> {
    let connection = open_database(app)?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS archive_export_state (
                owner_uin TEXT NOT NULL,
                target_path TEXT NOT NULL,
                last_export_at INTEGER NOT NULL,
                PRIMARY KEY(owner_uin, target_path)
            )",
        )
        .map_err(|error| format!("初始化增量导出记录失败：{error}"))?;
    let since = if incremental {
        connection
            .query_row(
                "SELECT last_export_at FROM archive_export_state WHERE owner_uin=?1 AND target_path=?2",
                params![owner_uin, target_path],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| format!("读取增量导出记录失败：{error}"))?
    } else {
        None
    };
    let images_dir = archived_image_dir(app, owner_uin)?;
    let media_files = Cell::new(0u64);
    let mut dynamic_count = 0u64;
    let mut html_files = 0u32;
    let mut nav_entries: Vec<(String, String, u64)> = Vec::new();
    let mut all_items: Vec<ArchiveItem> = Vec::new();
    let mut total_items = 0u64;
    for (category_id, _) in CATEGORIES
        .iter()
        .filter(|(id, _)| wanted.iter().any(|value| value == id))
    {
        total_items +=
            archive_items_for_export_since(&connection, owner_uin, category_id, None, since)?.len()
                as u64;
    }
    let album_total: u64 = if include_albums {
        connection
            .query_row(
                "SELECT COUNT(*) FROM archive_album_photos p
                 JOIN archive_albums a ON a.owner_uin=p.owner_uin AND a.album_id=p.album_id
                 WHERE p.owner_uin=?1 AND (?2 IS NULL OR a.updated_at >= ?2 OR EXISTS (
                     SELECT 1 FROM archive_album_photos p2
                     WHERE p2.owner_uin=a.owner_uin AND p2.album_id=a.album_id AND p2.updated_at >= ?2
                 ))",
                params![owner_uin, since],
                |row| row.get::<_, i64>(0),
            )
            .map(|count| count.max(0) as u64)
            .map_err(|error| format!("统计相册导出数量失败：{error}"))?
    } else {
        0
    };
    let total_work = total_items.saturating_add(album_total).max(1);
    state.set_progress(|progress| {
        progress.total = total_work;
        progress.message = if incremental && since.is_some() {
            format!("正在准备增量导出，共 {total_work} 项变更…")
        } else {
            format!("正在准备导出，共 {total_work} 项内容…")
        };
    });
    if state.is_cancelled() {
        return Err("EXPORT_CANCELLED".into());
    }

    for (category_id, category_name) in CATEGORIES
        .iter()
        .filter(|(id, _)| wanted.iter().any(|value| value == id))
    {
        if state.is_cancelled() {
            return Err("EXPORT_CANCELLED".into());
        }
        let items =
            archive_items_for_export_since(&connection, owner_uin, category_id, None, since)?;
        if items.is_empty() {
            continue;
        }
        dynamic_count += items.len() as u64;
        let picture = |item_index: usize, picture_index: usize, remote: &str| -> String {
            if !include_media {
                return remote.to_owned();
            }
            let id = items[item_index].id;
            if let Some(path) =
                existing_archived_image(&images_dir, &format!("{id}-{picture_index}"))
            {
                let file_name = format!("{id}-{picture_index}.{}", file_extension(&path));
                if copy_into(media_dir, &path, &file_name) {
                    media_files.set(media_files.get() + 1);
                    return format!("media/{file_name}");
                }
            }
            remote.to_owned()
        };
        let video = |item_index: usize, _remote: &str| -> Option<String> {
            if !include_media {
                return None;
            }
            let id = items[item_index].id;
            if let Ok(cache_path) = video_cache_path(app, owner_uin, id) {
                if cache_path.metadata().is_ok_and(|meta| meta.len() > 1024) {
                    let file_name = format!("{id}.mp4");
                    if copy_into(&media_dir.join("videos"), &cache_path, &file_name) {
                        media_files.set(media_files.get() + 1);
                        return Some(format!("media/videos/{file_name}"));
                    }
                }
            }
            None
        };
        let resolver = ExportMediaResolver {
            picture: &picture,
            video: &video,
        };
        let file_name = format!("{category_id}.html");
        let page = feed_document(&items, category_name, owner_uin, &resolver);
        fs::write(staging.join(&file_name), page)
            .map_err(|error| format!("写入分类导出页失败：{error}"))?;
        html_files += 1;
        advance_progress(
            state,
            items.len() as u64,
            format!("正在生成{}页面（{}条）…", category_name, items.len()),
        );
        nav_entries.push((file_name, (*category_name).to_owned(), items.len() as u64));
        all_items.extend(items);
    }

    let mut album_count = 0u32;
    let mut album_index: Vec<(String, String, u64)> = Vec::new();
    let mut album_records: Vec<serde_json::Value> = Vec::new();
    if include_albums {
        let albums = export_albums(
            owner_uin,
            &connection,
            staging,
            include_media,
            &media_files,
            since,
            state,
        )?;
        album_count = albums.len() as u32;
        for (dir_name, album_name, photo_count, record) in albums {
            album_index.push((
                format!("albums/{dir_name}/index.html"),
                album_name,
                photo_count,
            ));
            album_records.push(record);
        }
        if !album_index.is_empty() {
            let list = album_index
                .iter()
                .map(|(href, name, count)| {
                    format!(
                        "<li><a href=\"{}\">{}（{} 张）</a></li>",
                        html_escape(href),
                        html_escape(name),
                        count
                    )
                })
                .collect::<Vec<_>>()
                .join("");
            let page = format!(
                r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>QQ空间归档 - 相册</title><style>{css}ul{{padding-left:20px;line-height:2}}</style></head><body><main><h1>QQ空间归档 · 相册</h1><p class="intro">共 {count} 个相册 · <a href="../index.html">返回总目录</a></p><ul>{list}</ul></main></body></html>"#,
                css = EXPORT_CSS,
                count = album_index.len(),
                list = list,
            );
            fs::create_dir_all(staging.join("albums"))
                .map_err(|error| format!("无法创建相册导出目录：{error}"))?;
            fs::write(staging.join("albums/index.html"), page)
                .map_err(|error| format!("写入相册导出页失败：{error}"))?;
            html_files += 1;
        }
    }

    // 总目录页
    let nav = nav_entries
        .iter()
        .map(|(file, name, count)| {
            format!(
                "<li><a href=\"{}\">{}（{} 条）</a></li>",
                html_escape(file),
                html_escape(name),
                count
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let album_nav = if album_index.is_empty() {
        String::new()
    } else {
        format!(
            "<h2>相册</h2><ul>{}<li><a href=\"albums/index.html\">全部相册</a></li></ul>",
            album_index
                .iter()
                .map(|(href, name, count)| format!(
                    "<li><a href=\"{}\">{}（{} 张）</a></li>",
                    html_escape(href),
                    html_escape(name),
                    count
                ))
                .collect::<Vec<_>>()
                .join("")
        )
    };
    let exported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let index_page = format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>QQ空间归档离线包</title><style>{css}ul{{padding-left:20px;line-height:2}}</style></head><body><main><h1>QQ空间归档离线包</h1><p class="intro">账号 {owner} · 动态 {dynamics} 条 · 相册 {albums} 个 · 导出时间 <span id="export-time"></span></p><h2>动态</h2><ul>{nav}</ul>{album_nav}</main><script>document.querySelector('#export-time').textContent=new Date().toLocaleString();</script></body></html>"#,
        css = EXPORT_CSS,
        owner = html_escape(owner_uin),
        dynamics = dynamic_count,
        albums = album_count,
        nav = nav,
        album_nav = album_nav,
    );
    fs::write(staging.join("index.html"), index_page)
        .map_err(|error| format!("写入导出总目录失败：{error}"))?;
    html_files += 1;

    // 结构化数据：便于后续重新导入或迁移到其他工具
    let data = json!({
        "owner": owner_uin,
        "exportedAt": exported_at,
        "dynamics": all_items,
        "albums": album_records,
    });
    fs::write(
        staging.join("data.json"),
        serde_json::to_string_pretty(&data)
            .map_err(|error| format!("序列化结构化数据失败：{error}"))?,
    )
    .map_err(|error| format!("写入结构化数据失败：{error}"))?;

    if state.is_cancelled() {
        return Err("EXPORT_CANCELLED".into());
    }
    state.set_progress(|progress| {
        progress.message = "正在压缩离线包…".into();
    });
    let total_bytes = zip_directory(staging, target_path, state)?;
    connection
        .execute(
            "INSERT INTO archive_export_state(owner_uin,target_path,last_export_at)
             VALUES (?1,?2,?3)
             ON CONFLICT(owner_uin,target_path) DO UPDATE SET last_export_at=excluded.last_export_at",
            params![owner_uin, target_path, export_started_at],
        )
        .map_err(|error| format!("保存增量导出记录失败：{error}"))?;
    Ok(ExportPackageStats {
        target_path: target_path.to_owned(),
        dynamic_count,
        html_files,
        media_files: media_files.get(),
        album_count,
        total_bytes,
    })
}

type ExportedAlbum = (String, String, u64, serde_json::Value);

#[allow(clippy::too_many_arguments)]
fn export_albums(
    owner_uin: &str,
    connection: &rusqlite::Connection,
    staging: &Path,
    include_media: bool,
    media_files: &Cell<u64>,
    since: Option<i64>,
    state: &ExportState,
) -> Result<Vec<ExportedAlbum>, String> {
    let mut statement = connection
        .prepare(
            "SELECT a.album_id,COALESCE(a.name,'未命名相册'),a.photo_total,a.status,
                    p.id,p.pic_key,p.name,p.description,p.upload_time,p.preview_url,p.urls_json,
                    p.local_path,p.status
             FROM archive_albums a
             LEFT JOIN archive_album_photos p
               ON p.owner_uin=a.owner_uin AND p.album_id=a.album_id
             WHERE a.owner_uin=?1
               AND (?2 IS NULL OR a.updated_at >= ?2 OR EXISTS (
                   SELECT 1 FROM archive_album_photos p2
                   WHERE p2.owner_uin=a.owner_uin AND p2.album_id=a.album_id AND p2.updated_at >= ?2
               ))
             ORDER BY a.modify_time IS NULL, a.modify_time DESC,
                      p.upload_time IS NULL, p.upload_time ASC, p.id ASC",
        )
        .map_err(|error| format!("读取相册导出数据失败：{error}"))?;
    struct PhotoRow {
        pic_key: String,
        name: Option<String>,
        description: Option<String>,
        upload_time: Option<i64>,
        preview_url: Option<String>,
        urls: Vec<String>,
        local_path: Option<String>,
        status: String,
    }
    struct AlbumRow {
        album_id: String,
        name: String,
        photo_total: i64,
        status: String,
        photos: Vec<PhotoRow>,
    }
    let mut albums: Vec<AlbumRow> = Vec::new();
    let rows = statement
        .query_map(params![owner_uin, since], |row| {
            let urls_json: String = row.get::<_, Option<String>>(10)?.unwrap_or_default();
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
                urls_json,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
            ))
        })
        .map_err(|error| format!("读取相册导出数据失败：{error}"))?;
    for row in rows {
        let (
            album_id,
            name,
            photo_total,
            status,
            _photo_id,
            pic_key,
            photo_name,
            description,
            upload_time,
            preview_url,
            urls_json,
            local_path,
            photo_status,
        ) = row.map_err(|error| format!("读取相册导出数据失败：{error}"))?;
        let album = match albums.last_mut() {
            Some(album) if album.album_id == album_id => album,
            _ => {
                albums.push(AlbumRow {
                    album_id,
                    name,
                    photo_total,
                    status,
                    photos: Vec::new(),
                });
                albums.last_mut().expect("刚刚推入的相册必然存在")
            }
        };
        if let Some(pic_key) = pic_key {
            let urls: Vec<String> = serde_json::from_str(&urls_json).unwrap_or_default();
            album.photos.push(PhotoRow {
                pic_key,
                name: photo_name,
                description,
                upload_time,
                preview_url,
                urls,
                local_path,
                status: photo_status.unwrap_or_else(|| "pending".into()),
            });
        }
    }
    drop(statement);

    let mut exported: Vec<ExportedAlbum> = Vec::new();
    for album in &albums {
        if album.photos.is_empty() {
            continue;
        }
        let dir_name = crate::albums::safe_file_stem(&album.album_id);
        let album_dir = staging.join("albums").join(&dir_name);
        let photos_dir = album_dir.join("photos");
        fs::create_dir_all(&photos_dir).map_err(|error| format!("无法创建相册目录：{error}"))?;
        let mut tiles = String::new();
        let mut photo_records: Vec<serde_json::Value> = Vec::new();
        let mut copied_count = 0u64;
        for photo in &album.photos {
            if state.is_cancelled() {
                return Err("EXPORT_CANCELLED".into());
            }
            let stem = crate::albums::safe_file_stem(&photo.pic_key);
            let mut relative_src: Option<String> = None;
            if include_media {
                if let Some(local_path) = photo
                    .local_path
                    .as_deref()
                    .map(Path::new)
                    .filter(|path| path.metadata().is_ok_and(|meta| meta.len() > 32))
                {
                    let file_name = format!("{stem}.{}", file_extension(local_path));
                    if copy_into(&photos_dir, local_path, &file_name) {
                        media_files.set(media_files.get() + 1);
                        relative_src = Some(format!("photos/{file_name}"));
                        copied_count += 1;
                    }
                } else {
                    // 本地未缓存：尝试从本地动态图片缓存按 URL 命中不适用（相册照片独立存储），跳过
                }
            }
            let src = relative_src.clone().or_else(|| photo.preview_url.clone());
            let href = relative_src.clone().or_else(|| photo.urls.first().cloned());
            if let Some(src) = src {
                tiles.push_str(&format!(
                    "<figure class=\"album-photo\"><a href=\"{href}\" target=\"_blank\"><img loading=\"lazy\" referrerpolicy=\"no-referrer\" src=\"{src}\"></a><figcaption>{caption}</figcaption></figure>",
                    href = html_escape(href.as_deref().unwrap_or("#")),
                    src = html_escape(&src),
                    caption = html_escape(
                        photo.name.as_deref().or(photo.description.as_deref()).unwrap_or("")
                    ),
                ));
            }
            photo_records.push(json!({
                "picKey": photo.pic_key,
                "name": photo.name,
                "description": photo.description,
                "uploadTime": photo.upload_time,
                "status": photo.status,
                "exportedPath": relative_src,
                "urls": photo.urls,
            }));
        }
        let page = format!(
            r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>QQ空间归档 - {title}</title><style>{css}.album-photos{{display:grid;grid-template-columns:repeat(3,1fr);gap:10px}}.album-photo{{margin:0;background:#fff;border:1px solid #e5eaf2;border-radius:12px;overflow:hidden}}.album-photo img{{display:block;width:100%;height:180px;object-fit:cover}}.album-photo figcaption{{padding:7px 9px;color:#7e899a;font-size:12px;overflow-wrap:anywhere}}@media(max-width:600px){{.album-photos{{grid-template-columns:repeat(2,1fr)}}.album-photo img{{height:130px}}}}</style></head><body><main><h1>{title}</h1><p class="intro">共 {count} 张照片，其中 {copied} 张已随包离线保存 · <a href="../index.html">返回相册列表</a></p><div class="album-photos">{tiles}</div></main></body></html>"#,
            css = EXPORT_CSS,
            title = html_escape(&album.name),
            count = album.photos.len(),
            copied = copied_count,
            tiles = tiles,
        );
        fs::write(album_dir.join("index.html"), page)
            .map_err(|error| format!("写入相册导出页失败：{error}"))?;
        advance_progress(
            state,
            album.photos.len() as u64,
            format!("正在生成相册「{}」…", album.name),
        );
        exported.push((
            dir_name,
            album.name.clone(),
            album.photos.len() as u64,
            json!({
                "albumId": album.album_id,
                "name": album.name,
                "photoTotal": album.photo_total,
                "status": album.status,
                "photos": photo_records,
            }),
        ));
    }
    Ok(exported)
}

fn copy_into(target_dir: &Path, source: &Path, file_name: &str) -> bool {
    if fs::create_dir_all(target_dir).is_err() {
        return false;
    }
    let target = target_dir.join(file_name);
    if target.exists() {
        return true;
    }
    fs::copy(source, target).is_ok()
}

fn file_extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("jpg")
        .to_ascii_lowercase()
}

fn zip_directory(source: &Path, target: &str, state: &ExportState) -> Result<u64, String> {
    use std::io::Write as _;
    let partial = PathBuf::from(format!("{target}.part"));
    let _ = fs::remove_file(&partial);
    let result = (|| {
        let file =
            fs::File::create(&partial).map_err(|error| format!("无法创建导出文件：{error}"))?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut stack = vec![source.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries =
                fs::read_dir(&dir).map_err(|error| format!("读取导出目录失败：{error}"))?;
            for entry in entries {
                if state.is_cancelled() {
                    return Err("EXPORT_CANCELLED".into());
                }
                let entry = entry.map_err(|error| format!("读取导出目录失败：{error}"))?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(source)
                    .map_err(|error| format!("计算导出相对路径失败：{error}"))?;
                let name = relative.to_string_lossy().replace('\\', "/");
                if path.is_dir() {
                    stack.push(path);
                    zip.add_directory(&name, options)
                        .map_err(|error| format!("写入 zip 目录失败：{error}"))?;
                } else {
                    zip.start_file(&name, options)
                        .map_err(|error| format!("写入 zip 失败：{error}"))?;
                    let bytes =
                        fs::read(&path).map_err(|error| format!("读取待打包文件失败：{error}"))?;
                    zip.write_all(&bytes)
                        .map_err(|error| format!("写入 zip 数据失败：{error}"))?;
                }
            }
        }
        zip.finish()
            .map_err(|error| format!("完成 zip 打包失败：{error}"))?;
        fs::metadata(&partial)
            .map(|metadata| metadata.len())
            .map_err(|error| format!("读取导出文件大小失败：{error}"))
    })();
    match result {
        Ok(total_bytes) => {
            if state.is_cancelled() {
                let _ = fs::remove_file(&partial);
                return Err("EXPORT_CANCELLED".into());
            }
            let target_path = Path::new(target);
            let backup = PathBuf::from(format!("{target}.previous"));
            let _ = fs::remove_file(&backup);
            if target_path.exists() {
                fs::rename(target_path, &backup)
                    .map_err(|error| format!("暂存旧导出文件失败：{error}"))?;
            }
            if let Err(error) = fs::rename(&partial, target_path) {
                if backup.exists() {
                    let _ = fs::rename(&backup, target_path);
                }
                return Err(format!("保存导出文件失败：{error}"));
            }
            let _ = fs::remove_file(&backup);
            Ok(total_bytes)
        }
        Err(error) => {
            let _ = fs::remove_file(&partial);
            Err(error)
        }
    }
}

#[tauri::command]
pub fn get_export_progress(state: tauri::State<'_, ExportState>) -> Result<ExportProgress, String> {
    state.snapshot()
}

#[tauri::command]
pub fn cancel_export(state: tauri::State<'_, ExportState>) {
    state.cancel.store(true, Ordering::Relaxed);
}
