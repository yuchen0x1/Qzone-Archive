use serde::Serialize;
use tauri::AppHandle;
use url::Url;

const RELEASE_API: &str = "https://gitee.com/api/v5/repos/yuchen0x1/Qzone-Archive/releases/latest";
const RELEASES_PAGE_URL: &str = "https://gitee.com/yuchen0x1/Qzone-Archive/releases";
const ALLOWED_UPDATE_HOSTS: [&str; 6] = [
    "api.github.com",
    "github.com",
    "gitee.com",
    "api.gitee.com",
    "gitcode.com",
    "api.gitcode.com",
];

#[derive(Serialize)]
pub struct UpdateAsset {
    pub name: String,
    pub size: u64,
    pub browser_download_url: String,
}

#[derive(Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub notes: String,
    pub release_url: String,
    pub published_at: String,
    pub assets: Vec<UpdateAsset>,
}

/// 仅允许 https 且 host 在白名单内，拒绝 localhost / 环回 / 内网等地址。
fn validate_release_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|e| format!("无效的更新接口地址: {e}"))?;
    if url.scheme() != "https" {
        return Err("更新接口仅允许 https".into());
    }
    let host = url.host_str().unwrap_or_default();
    if !ALLOWED_UPDATE_HOSTS
        .iter()
        .any(|h| host.eq_ignore_ascii_case(h))
    {
        return Err(format!("更新接口主机不被允许: {host}"));
    }
    Ok(url)
}

fn configured_value(value: Option<&'static str>, fallback: &'static str) -> &'static str {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
}

fn configured_update_api() -> &'static str {
    configured_value(option_env!("QZONE_UPDATE_API_URL"), RELEASE_API)
}

fn configured_releases_page() -> &'static str {
    configured_value(option_env!("QZONE_RELEASES_URL"), RELEASES_PAGE_URL)
}

fn is_trusted_source_url(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            ALLOWED_UPDATE_HOSTS
                .iter()
                .any(|allowed| host.eq_ignore_ascii_case(allowed))
        })
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn safe_release_page_url(raw: &str, tag: &str) -> String {
    let base = configured_releases_page().trim_end_matches('/');
    if is_trusted_source_url(raw) && raw.starts_with(&format!("{base}/")) {
        return raw.to_string();
    }
    format!("{base}/tag/{tag}")
}

fn version_tuple(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.split('-').next().unwrap_or("0").parse().unwrap_or(0))
        .collect()
}

fn is_newer(latest: &str, current: &str) -> bool {
    let (latest_parts, current_parts) = (version_tuple(latest), version_tuple(current));
    for i in 0..latest_parts.len().max(current_parts.len()) {
        let l = latest_parts.get(i).copied().unwrap_or(0);
        let c = current_parts.get(i).copied().unwrap_or(0);
        if l != c {
            return l > c;
        }
    }
    false
}

fn parse_update_info(
    release: &serde_json::Value,
    current: String,
) -> Result<Option<UpdateInfo>, String> {
    let tag = release["tag_name"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if tag.is_empty() {
        return Err("仓库还没有发布任何版本".into());
    }
    if !is_newer(&tag, &current) {
        return Ok(None);
    }

    let assets = release["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|asset| {
                    let name = asset["name"].as_str()?.to_string();
                    let browser_download_url = asset["browser_download_url"].as_str()?.to_string();
                    if !is_trusted_source_url(&browser_download_url) {
                        return None;
                    }
                    Some(UpdateAsset {
                        name,
                        size: asset["size"].as_u64().unwrap_or(0),
                        browser_download_url,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Some(UpdateInfo {
        current_version: current,
        latest_version: tag.trim_start_matches('v').to_string(),
        notes: release["body"].as_str().unwrap_or_default().to_string(),
        release_url: safe_release_page_url(release["html_url"].as_str().unwrap_or_default(), &tag),
        // Gitee 的 latest 接口 published_at 可能为 null，回退到 created_at
        published_at: release["published_at"]
            .as_str()
            .or_else(|| release["created_at"].as_str())
            .unwrap_or_default()
            .to_string(),
        assets,
    }))
}

/// 查询发布仓库最新 Release；有新版本时返回详细信息，已是最新则返回 None。
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let url = validate_release_url(configured_update_api())?;
    let client = reqwest::Client::builder()
        .user_agent("qzonearchive-updater")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("初始化网络客户端失败: {e}"))?;
    let release: serde_json::Value = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("发布仓库接口返回错误: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析更新信息失败: {e}"))?;

    let current = app.package_info().version.to_string();
    parse_update_info(&release, current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semantic_versions() {
        assert!(is_newer("v1.0.4", "1.0.3"));
        assert!(is_newer("1.1.0", "1.0.9"));
        assert!(is_newer("2.0", "1.9.9"));
        assert!(!is_newer("1.0.3", "1.0.3"));
        assert!(!is_newer("1.0.3", "1.0.4"));
    }

    #[test]
    fn rejects_unallowed_update_hosts() {
        assert!(validate_release_url(RELEASE_API).is_ok());
        assert!(
            validate_release_url("https://gitee.com/api/v5/repos/example/app/releases/latest")
                .is_ok()
        );
        assert!(validate_release_url("http://api.github.com/repos/x/y/releases/latest").is_err());
        assert!(validate_release_url("https://127.0.0.1/x").is_err());
        assert!(validate_release_url("https://localhost/x").is_err());
        assert!(validate_release_url("https://evil.example.com/x").is_err());
    }

    #[test]
    fn only_accepts_trusted_https_release_urls() {
        assert!(is_trusted_source_url(
            "https://github.com/yuchen0x1/Qzone-Archive/releases/download/v1.0.7/app.exe"
        ));
        assert!(is_trusted_source_url(
            "https://gitee.com/example/qzone/releases/download/v1.0.7/app.exe"
        ));
        assert!(!is_trusted_source_url(
            "http://github.com/yuchen0x1/Qzone-Archive/releases/download/v1.0.7/app.exe"
        ));
        assert!(!is_trusted_source_url(
            "https://github.com:444/yuchen0x1/Qzone-Archive/releases/download/v1.0.7/app.exe"
        ));
        assert!(!is_trusted_source_url(
            "https://evil.example.com/yuchen0x1/Qzone-Archive/releases/download/v1.0.7/app.exe"
        ));
    }

    #[test]
    fn ignores_empty_build_overrides() {
        assert_eq!(configured_value(Some(""), "fallback"), "fallback");
        assert_eq!(configured_value(Some("  "), "fallback"), "fallback");
        assert_eq!(
            configured_value(Some("https://gitee.com/x"), "fallback"),
            "https://gitee.com/x"
        );
    }

    #[test]
    fn parses_new_release_and_keeps_only_safe_assets() {
        let release = serde_json::json!({
            "tag_name": "v1.0.7",
            "body": "修复更新入口",
            "html_url": "https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.0.7",
            "published_at": "2026-08-29T00:00:00Z",
            "assets": [
                {"name": "QzoneArchive_1.0.7_x64-setup.exe", "size": 42, "browser_download_url": "https://github.com/yuchen0x1/Qzone-Archive/releases/download/v1.0.7/QzoneArchive_1.0.7_x64-setup.exe"},
                {"name": "malicious.exe", "size": 99, "browser_download_url": "https://evil.example.com/malicious.exe"}
            ]
        });
        let info = parse_update_info(&release, "1.0.6".into())
            .unwrap()
            .unwrap();
        assert_eq!(info.latest_version, "1.0.7");
        assert_eq!(info.assets.len(), 1);
        assert_eq!(info.assets[0].name, "QzoneArchive_1.0.7_x64-setup.exe");
    }

    #[test]
    fn returns_none_when_release_is_current() {
        let release = serde_json::json!({"tag_name": "v1.0.6", "assets": []});
        assert!(parse_update_info(&release, "1.0.6".into())
            .unwrap()
            .is_none());
    }

    #[test]
    fn falls_back_to_the_canonical_release_page_for_untrusted_html_url() {
        let release = serde_json::json!({
            "tag_name": "v1.0.7",
            "html_url": "https://evil.example.com/update",
            "assets": []
        });
        let info = parse_update_info(&release, "1.0.6".into())
            .unwrap()
            .unwrap();
        assert_eq!(
            info.release_url,
            "https://gitee.com/yuchen0x1/Qzone-Archive/releases/tag/v1.0.7"
        );
    }
}
