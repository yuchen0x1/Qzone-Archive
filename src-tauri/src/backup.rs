use std::{
    fs,
    io::{Read as IoRead, Write as IoWrite},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde::Serialize;
use tauri::Manager;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use crate::{
    archive::{database_path, ArchiveState},
    qlogin::QLoginState,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStats {
    target_path: String,
    database_bytes: u64,
    image_count: u32,
    image_bytes: u64,
    total_bytes: u64,
}

fn ensure_archive_idle(state: &ArchiveState) -> Result<(), String> {
    if state.is_feed_archive_running() {
        return Err("归档任务运行时不能备份或恢复数据，请先取消任务".into());
    }
    Ok(())
}

/// 将 WAL/SHM 日志合并回主库，保证打包出的数据库文件完整自足。
fn checkpoint_database(app: &tauri::AppHandle) -> Result<(), String> {
    let database = database_path(app)?;
    if !database.exists() {
        return Ok(());
    }
    let connection = Connection::open(&database)
        .map_err(|error| format!("无法打开归档数据库以合并日志：{error}"))?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| format!("无法切换归档数据库日志模式：{error}"))?;
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| format!("无法合并归档数据库日志：{error}"))?;
    Ok(())
}

fn add_entry<W>(zip: &mut ZipWriter<W>, rel_path: &str, bytes: &[u8]) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
{
    zip.start_file(
        rel_path,
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )
    .map_err(|error| format!("无法写入备份文件：{error}"))?;
    zip.write_all(bytes)
        .map_err(|error| format!("无法写入备份内容：{error}"))
}

/// 图片本身已是压缩格式（JPEG/PNG/WebP/GIF/AVIF/MP4），再 deflate 只会消耗 CPU，
/// 压缩率接近 0，因此备份媒体时使用 Stored。
fn add_media_entry<W>(zip: &mut ZipWriter<W>, rel_path: &str, bytes: &[u8]) -> Result<(), String>
where
    W: std::io::Write + std::io::Seek,
{
    zip.start_file(
        rel_path,
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )
    .map_err(|error| format!("无法写入备份文件：{error}"))?;
    zip.write_all(bytes)
        .map_err(|error| format!("无法写入备份内容：{error}"))
}

/// 备份全部本地归档数据：SQLite 数据库 + 归档图片目录，打包为单个 .qzbak 文件。
#[tauri::command]
pub async fn backup_app_data(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    state: tauri::State<'_, ArchiveState>,
    target_path: String,
) -> Result<BackupStats, String> {
    ensure_archive_idle(&state)?;
    let _owner = login.qzone_auth().await?;
    if target_path.trim().is_empty() {
        return Err("请先选择备份文件保存位置".into());
    }
    checkpoint_database(&app)?;

    let database = database_path(&app)?;
    let images_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取图片归档目录：{error}"))?
        .join("images");
    let saved_path = target_path.trim().to_string();
    let moved_path = saved_path.clone();

    let (database_bytes, image_count, image_bytes) =
        tauri::async_runtime::spawn_blocking(move || {
            build_backup(&moved_path, &database, &images_dir)
        })
        .await
        .map_err(|error| format!("备份任务失败：{error}"))??;

    Ok(BackupStats {
        target_path: saved_path,
        database_bytes,
        image_count,
        image_bytes,
        total_bytes: database_bytes + image_bytes,
    })
}

/// 递归收集图片归档目录下的全部文件，返回 (绝对路径, 相对路径) 列表。
/// 图片按账号与相册分层存放：images/<uin>/…、images/<uin>/albums/<album_id>/…，
/// 只扫顶层会漏掉全部媒体。符号链接一律跳过，避免把归档目录之外的文件写进备份。
fn collect_media_files(images_dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    let mut files = Vec::new();
    if !images_dir.exists() {
        return Ok(files);
    }
    let mut pending = vec![images_dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|error| format!("无法读取图片归档目录：{error}"))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("无法读取图片归档目录：{error}"))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("无法读取归档文件属性：{error}"))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let Ok(relative) = path.strip_prefix(images_dir) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if relative.is_empty() {
                continue;
            }
            files.push((path, relative));
        }
    }
    Ok(files)
}

fn build_backup(
    target_path: &str,
    database: &Path,
    images_dir: &Path,
) -> Result<(u64, u32, u64), String> {
    if !database.exists() {
        return Err("归档数据库不存在，无法创建备份".into());
    }
    let partial = PathBuf::from(format!("{target_path}.part"));
    let _ = fs::remove_file(&partial);
    let build_result = (|| {
        let file =
            fs::File::create(&partial).map_err(|error| format!("无法创建备份文件：{error}"))?;
        let mut zip = ZipWriter::new(file);

        let bytes = fs::read(database).map_err(|error| format!("无法读取归档数据库：{error}"))?;
        let database_bytes = bytes.len() as u64;
        add_entry(&mut zip, "qzone-archive.sqlite3", &bytes)?;

        let mut image_count = 0u32;
        let mut image_bytes = 0u64;
        for (path, relative) in collect_media_files(images_dir)? {
            let bytes =
                fs::read(&path).map_err(|error| format!("无法读取归档图片 {relative}：{error}"))?;
            image_bytes += bytes.len() as u64;
            image_count += 1;
            add_media_entry(&mut zip, &format!("images/{relative}"), &bytes)?;
        }

        zip.finish()
            .map_err(|error| format!("无法完成备份文件写入：{error}"))?;
        Ok((database_bytes, image_count, image_bytes))
    })();
    if let Err(error) = build_result {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    let (database_bytes, image_count, image_bytes) = build_result.expect("上面已处理失败分支");
    let target = Path::new(target_path);
    let backup = PathBuf::from(format!("{target_path}.previous"));
    let _ = fs::remove_file(&backup);
    if target.exists() {
        fs::rename(target, &backup).map_err(|error| format!("暂存旧备份文件失败：{error}"))?;
    }
    if let Err(error) = fs::rename(&partial, target) {
        if backup.exists() {
            let _ = fs::rename(&backup, target);
        }
        let _ = fs::remove_file(&partial);
        return Err(format!("保存备份文件失败：{error}"));
    }
    let _ = fs::remove_file(&backup);
    Ok((database_bytes, image_count, image_bytes))
}

/// 从备份文件恢复：先完整解压到暂存目录并校验数据库完整性，校验通过后才替换现有数据，失败自动回滚。
#[tauri::command]
pub async fn restore_app_data(
    app: tauri::AppHandle,
    login: tauri::State<'_, QLoginState>,
    state: tauri::State<'_, ArchiveState>,
    backup_path: String,
) -> Result<BackupStats, String> {
    ensure_archive_idle(&state)?;
    let _owner = login.qzone_auth().await?;
    if backup_path.trim().is_empty() {
        return Err("请先选择备份文件".into());
    }

    let database = database_path(&app)?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法获取应用数据目录：{error}"))?;
    let images_dir = data_dir.join("images");
    let backup_path = PathBuf::from(backup_path.trim());

    tauri::async_runtime::spawn_blocking(move || {
        apply_restore(&backup_path, &database, &images_dir, &data_dir)
    })
    .await
    .map_err(|error| format!("恢复任务失败：{error}"))?
}

fn apply_restore(
    backup_path: &Path,
    database: &Path,
    images_dir: &Path,
    data_dir: &Path,
) -> Result<BackupStats, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = data_dir.join(format!("restore-{nonce}"));
    let staging_images = staging.join("images");
    let rollback = staging.join("rollback");
    let rollback_images = rollback.join("images");
    fs::create_dir_all(&staging_images)
        .map_err(|error| format!("创建恢复暂存目录失败：{error}"))?;
    let prepare_result = (|| {
        let mut archive = zip::ZipArchive::new(
            fs::File::open(backup_path).map_err(|error| format!("无法打开备份文件：{error}"))?,
        )
        .map_err(|error| format!("无法读取备份文件：{error}"))?;
        if archive.len() > 100_000 {
            return Err("备份文件条目数量异常，已中止恢复".into());
        }
        let mut database_bytes = 0u64;
        let mut image_count = 0u32;
        let mut image_bytes = 0u64;
        let mut database_found = false;
        for index in 0..archive.len() {
            let mut file = archive
                .by_index(index)
                .map_err(|error| format!("无法读取备份条目：{error}"))?;
            let name = file
                .name()
                .map_err(|error| format!("无法读取备份条目名称：{error}"))?
                .to_string();
            let uncompressed_size = file.size();
            if uncompressed_size > 8 * 1024 * 1024 * 1024 {
                return Err("备份条目过大，已中止恢复".into());
            }
            let mut bytes = Vec::new();
            let mut limited = (&mut file).take(8 * 1024 * 1024 * 1024 + 1);
            std::io::Read::read_to_end(&mut limited, &mut bytes)
                .map_err(|error| format!("无法读取备份内容 {name}：{error}"))?;
            if bytes.len() as u64 > uncompressed_size {
                return Err("备份内容长度异常，已中止恢复".into());
            }
            if name == "qzone-archive.sqlite3" {
                let target = staging.join("qzone-archive.sqlite3");
                fs::write(&target, &bytes)
                    .map_err(|error| format!("无法写入恢复数据库：{error}"))?;
                database_bytes = bytes.len() as u64;
                database_found = true;
            } else if let Some(rel) = name.strip_prefix("images/") {
                validate_relative_path(rel)?;
                let target = staging_images.join(rel);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("无法创建恢复图片目录：{error}"))?;
                }
                fs::write(&target, &bytes)
                    .map_err(|error| format!("无法写入恢复图片 {rel}：{error}"))?;
                image_bytes += bytes.len() as u64;
                image_count += 1;
            } else {
                continue;
            }
        }
        if !database_found {
            return Err("备份文件缺少归档数据库，已中止恢复".into());
        }
        validate_database(&staging.join("qzone-archive.sqlite3"))?;
        Ok(BackupStats {
            target_path: backup_path.to_string_lossy().into_owned(),
            database_bytes,
            image_count,
            image_bytes,
            total_bytes: database_bytes + image_bytes,
        })
    })();
    let stats = match prepare_result {
        Ok(stats) => stats,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };

    let replace_result = (|| {
        fs::create_dir_all(&rollback).map_err(|error| format!("创建回滚目录失败：{error}"))?;
        if database.exists() {
            fs::rename(database, rollback.join("qzone-archive.sqlite3"))
                .map_err(|error| format!("暂存旧数据库失败：{error}"))?;
        }
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{suffix}", database.display()));
            if sidecar.exists() {
                let _ = fs::rename(
                    &sidecar,
                    rollback.join(format!("qzone-archive.sqlite3{suffix}")),
                );
            }
        }
        if images_dir.exists() {
            fs::rename(images_dir, &rollback_images)
                .map_err(|error| format!("暂存旧图片归档失败：{error}"))?;
        }
        if let Err(error) = fs::rename(staging.join("qzone-archive.sqlite3"), database) {
            let _ = fs::rename(rollback.join("qzone-archive.sqlite3"), database);
            return Err(format!("恢复数据库失败：{error}"));
        }
        let restore_images = if staging_images.exists() {
            fs::rename(&staging_images, images_dir)
                .map_err(|error| format!("恢复图片归档失败：{error}"))?;
            true
        } else {
            fs::create_dir_all(images_dir)
                .map_err(|error| format!("无法重建图片归档目录：{error}"))?;
            false
        };
        if let Err(error) = validate_database(database) {
            let _ = fs::remove_file(database);
            let _ = fs::rename(rollback.join("qzone-archive.sqlite3"), database);
            if restore_images {
                let _ = fs::remove_dir_all(images_dir);
                if rollback_images.exists() {
                    let _ = fs::rename(&rollback_images, images_dir);
                }
            } else if rollback_images.exists() {
                let _ = fs::remove_dir_all(images_dir);
                let _ = fs::rename(&rollback_images, images_dir);
            }
            return Err(error);
        }
        let _ = fs::remove_dir_all(&rollback);
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging);
    replace_result?;
    Ok(stats)
}

fn validate_relative_path(relative: &str) -> Result<(), String> {
    if relative.is_empty()
        || Path::new(relative).is_absolute()
        || relative.starts_with('/')
        || relative.starts_with('\\')
        || relative.split('\\').any(|part| part == "..")
        || relative.split('/').any(|part| part == "..")
    {
        return Err("备份图片路径非法，已中止恢复".into());
    }
    Ok(())
}

fn validate_database(database: &Path) -> Result<(), String> {
    let connection = rusqlite::Connection::open(database)
        .map_err(|error| format!("恢复数据库校验失败：{error}"))?;
    connection
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .map_err(|error| format!("恢复数据库校验失败：{error}"))
        .and_then(|result| {
            if result == "ok" {
                Ok(())
            } else {
                Err(format!("恢复数据库校验失败：{result}"))
            }
        })
}

#[cfg(test)]
mod tests {
    use super::{collect_media_files, validate_relative_path};
    use std::fs;

    #[test]
    fn accepts_nested_image_paths() {
        assert!(validate_relative_path("2026/album/photo.jpg").is_ok());
    }

    #[test]
    fn rejects_traversal_and_absolute_image_paths() {
        assert!(validate_relative_path("../outside.jpg").is_err());
        assert!(validate_relative_path("nested/../../outside.jpg").is_err());
        assert!(validate_relative_path("C:\\outside.jpg").is_err());
        assert!(validate_relative_path("/outside.jpg").is_err());
        assert!(validate_relative_path("\\outside.jpg").is_err());
    }

    #[test]
    fn collects_media_files_from_nested_account_and_album_dirs() {
        let root = std::env::temp_dir().join(format!(
            "qzone-backup-test-{}",
            std::process::id() as u128 * 1000 + 7
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("10001/albums/album-a")).unwrap();
        fs::write(root.join("10001/12-0.jpg"), b"dynamic-photo").unwrap();
        fs::write(root.join("10001/albums/album-a/pic-1.jpg"), b"album-photo").unwrap();

        let mut files = collect_media_files(&root).unwrap();
        files.sort_by(|left, right| left.1.cmp(&right.1));
        let relatives: Vec<&str> = files.iter().map(|(_, rel)| rel.as_str()).collect();
        assert_eq!(
            relatives,
            vec!["10001/12-0.jpg", "10001/albums/album-a/pic-1.jpg"]
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_images_dir_collects_nothing() {
        let root = std::env::temp_dir().join("qzone-backup-test-missing");
        let _ = fs::remove_dir_all(&root);
        assert!(collect_media_files(&root).unwrap().is_empty());
    }

    /// 前端用 convertFileSrc 加载本地图片/视频，CSP 必须放行 asset 协议，
    /// 否则本地媒体会被静默拦截并回退到远程地址（断网即失效）。
    #[test]
    fn csp_allows_asset_protocol_for_media() {
        let config = include_str!("../tauri.conf.json");
        let value: serde_json::Value = serde_json::from_str(config).unwrap();
        let csp = value["app"]["security"]["csp"].as_str().unwrap_or_default();
        for directive in ["img-src", "media-src", "connect-src"] {
            let start = csp
                .split(';')
                .map(str::trim)
                .find(|part| part.starts_with(&format!("{directive} ")))
                .unwrap_or_else(|| panic!("CSP 缺少 {directive} 指令"));
            assert!(
                start.contains("asset:"),
                "{directive} 缺少 asset: 协议，本地媒体会被 CSP 拦截"
            );
        }
    }
}
