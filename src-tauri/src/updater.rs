use serde::Serialize;
use tauri::AppHandle;
use url::Url;

/// 一个发布源：latest 接口 + 对应发布页（用于校验发布链接）。
struct ReleaseSource {
    name: &'static str,
    api: &'static str,
    releases_page: &'static str,
    /// API 被限流/不可达时的 releases.atom 订阅兜底地址，无则不启用。
    atom_fallback: Option<&'static str>,
}

const GITHUB_SOURCE: ReleaseSource = ReleaseSource {
    name: "GitHub",
    api: "https://api.github.com/repos/yuchen0x1/Qzone-Archive/releases/latest",
    releases_page: "https://github.com/yuchen0x1/Qzone-Archive/releases",
    atom_fallback: Some("https://github.com/yuchen0x1/Qzone-Archive/releases.atom"),
};

const GITEE_SOURCE: ReleaseSource = ReleaseSource {
    name: "Gitee",
    api: "https://gitee.com/api/v5/repos/yuchen0x1/Qzone-Archive/releases/latest",
    releases_page: "https://gitee.com/yuchen0x1/Qzone-Archive/releases",
    atom_fallback: None,
};

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

/// 待尝试的发布源：构建时配置了 QZONE_UPDATE_API_URL 则只用该源，否则依次尝试内置源。
fn effective_release_sources() -> Vec<ReleaseSource> {
    if let Some(api) = option_env!("QZONE_UPDATE_API_URL").filter(|v| !v.trim().is_empty()) {
        return vec![ReleaseSource {
            name: "构建时配置",
            api,
            releases_page: option_env!("QZONE_RELEASES_URL")
                .filter(|v| !v.trim().is_empty())
                .unwrap_or(GITHUB_SOURCE.releases_page),
            atom_fallback: None,
        }];
    }
    vec![GITHUB_SOURCE, GITEE_SOURCE]
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

fn safe_release_page_url(raw: &str, tag: &str, releases_page: &str) -> String {
    let base = releases_page.trim_end_matches('/');
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
    releases_page: &str,
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
        release_url: safe_release_page_url(
            release["html_url"].as_str().unwrap_or_default(),
            &tag,
            releases_page,
        ),
        // 部分发布平台的 latest 接口 published_at 可能为 null，回退到 created_at
        published_at: release["published_at"]
            .as_str()
            .or_else(|| release["created_at"].as_str())
            .unwrap_or_default()
            .to_string(),
        assets,
    }))
}

/// 查询发布仓库最新 Release；有新版本时返回详细信息，已是最新则返回 None。
/// 依次尝试各发布源，单个源失败（限流/网络/接口错误）时自动回退下一个。
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let current = app.package_info().version.to_string();
    let client = reqwest::Client::builder()
        .user_agent("qzonearchive-updater")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("初始化网络客户端失败: {e}"))?;

    let mut failures: Vec<String> = Vec::new();
    for source in effective_release_sources() {
        match fetch_api_release(&client, &source, &current).await {
            Ok(info) => return Ok(info),
            Err(err) => failures.push(format!("{} API: {err}", source.name)),
        }
        if let Some(feed) = source.atom_fallback {
            match fetch_atom_release(&client, feed, source.releases_page, &current).await {
                Ok(info) => return Ok(info),
                Err(err) => failures.push(format!("{} 订阅: {err}", source.name)),
            }
        }
    }
    Err(format!("所有发布源均不可达（{}）", failures.join("；")))
}

/// 调用发布源的 latest 接口并解析结果。
async fn fetch_api_release(
    client: &reqwest::Client,
    source: &ReleaseSource,
    current: &str,
) -> Result<Option<UpdateInfo>, String> {
    let url = validate_release_url(source.api)?;
    let release: serde_json::Value = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("接口返回错误: {e}"))?
        .json()
        .await
        .map_err(|e| format!("解析更新信息失败: {e}"))?;
    parse_update_info(&release, current.to_string(), source.releases_page)
}

/// API 不可用时读取 releases.atom 订阅获取最新版本（订阅不含资产列表与预发布标记）。
async fn fetch_atom_release(
    client: &reqwest::Client,
    feed_url: &str,
    releases_page: &str,
    current: &str,
) -> Result<Option<UpdateInfo>, String> {
    let url = validate_release_url(feed_url)?;
    let body = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("订阅请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("发布订阅返回错误: {e}"))?
        .text()
        .await
        .map_err(|e| format!("读取发布订阅失败: {e}"))?;
    let (tag, published_at) = parse_atom_latest(&body)?;
    if !is_newer(&tag, current) {
        return Ok(None);
    }
    Ok(Some(UpdateInfo {
        current_version: current.to_string(),
        latest_version: tag.trim_start_matches('v').to_string(),
        notes: String::new(),
        release_url: format!("{}/tag/{tag}", releases_page.trim_end_matches('/')),
        published_at,
        assets: Vec::new(),
    }))
}

/// 从 releases.atom 订阅中提取最新版本的标签与更新时间；订阅按发布时间倒序，取首个 entry。
fn parse_atom_latest(body: &str) -> Result<(String, String), String> {
    let entry = body.split("<entry>").nth(1).ok_or("发布源还没有任何版本")?;
    // entry 内第一个 href 指向最新版本的发布页（…/releases/tag/<tag>）；正文中的引号已转义不会误配
    let href = entry
        .split("href=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or("发布订阅内容缺少版本链接")?;
    let tag = href
        .split_once("/releases/tag/")
        .map(|(_, tag)| tag)
        .filter(|tag| !tag.is_empty())
        .ok_or("发布订阅内容缺少版本链接")?;
    let published_at = entry
        .split("<updated>")
        .nth(1)
        .and_then(|rest| rest.split("</updated>").next())
        .unwrap_or_default()
        .to_string();
    Ok((tag.to_string(), published_at))
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
        assert!(validate_release_url(GITHUB_SOURCE.api).is_ok());
        assert!(validate_release_url(GITEE_SOURCE.api).is_ok());
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
    fn tries_builtin_sources_in_order() {
        let sources = effective_release_sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].name, "GitHub");
        assert_eq!(sources[1].name, "Gitee");
        assert!(sources[0].atom_fallback.is_some());
        assert!(sources[1].atom_fallback.is_none());
        for source in &sources {
            assert!(validate_release_url(source.api).is_ok());
        }
    }

    #[test]
    fn parses_latest_tag_from_atom_feed() {
        let feed = r#"<feed xmlns="http://www.w3.org/2005/Atom">
  <id>tag:github.com,2008:https://github.com/o/r/releases</id>
  <updated>2026-10-21T11:00:50Z</updated>
  <entry>
    <id>tag:github.com,2008:Repository/196701619/v1.2.0</id>
    <updated>2026-08-29T01:02:03Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v1.2.0"/>
    <title>Release v1.2.0</title>
    <content type="html">&lt;p&gt;see &lt;a href=&quot;https://example.com&quot;&gt;x&lt;/a&gt;&lt;/p&gt;</content>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/196701619/v1.1.0</id>
    <updated>2026-07-01T00:00:00Z</updated>
    <link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v1.1.0"/>
    <title>Release v1.1.0</title>
  </entry>
</feed>"#;
        let (tag, published_at) = parse_atom_latest(feed).unwrap();
        assert_eq!(tag, "v1.2.0");
        assert_eq!(published_at, "2026-08-29T01:02:03Z");
        assert!(parse_atom_latest("<feed><updated>x</updated></feed>").is_err());
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
        let info = parse_update_info(&release, "1.0.6".into(), GITHUB_SOURCE.releases_page)
            .unwrap()
            .unwrap();
        assert_eq!(info.latest_version, "1.0.7");
        assert_eq!(info.assets.len(), 1);
        assert_eq!(info.assets[0].name, "QzoneArchive_1.0.7_x64-setup.exe");
    }

    #[test]
    fn returns_none_when_release_is_current() {
        let release = serde_json::json!({"tag_name": "v1.0.6", "assets": []});
        assert!(
            parse_update_info(&release, "1.0.6".into(), GITHUB_SOURCE.releases_page)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn falls_back_to_the_canonical_release_page_for_untrusted_html_url() {
        let release = serde_json::json!({
            "tag_name": "v1.0.7",
            "html_url": "https://evil.example.com/update",
            "assets": []
        });
        let info = parse_update_info(&release, "1.0.6".into(), GITHUB_SOURCE.releases_page)
            .unwrap()
            .unwrap();
        assert_eq!(
            info.release_url,
            "https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.0.7"
        );
    }

    #[test]
    fn falls_back_to_the_gitee_page_when_source_is_gitee() {
        let release = serde_json::json!({
            "tag_name": "v1.2.1",
            "html_url": "https://evil.example.com/update",
            "assets": []
        });
        let info = parse_update_info(&release, "1.2.0".into(), GITEE_SOURCE.releases_page)
            .unwrap()
            .unwrap();
        assert_eq!(
            info.release_url,
            "https://gitee.com/yuchen0x1/Qzone-Archive/releases/tag/v1.2.1"
        );
    }
}
