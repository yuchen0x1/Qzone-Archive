# 发布记录

项目于 **2026-11-13（Asia/Shanghai）停止维护并转为存档**，最后发布版本为 **v1.3.5**。本次仅整理文档与仓库，不新增版本或重建历史安装包。

以下覆盖当前重写版本线的全部 9 个版本，按版本标签及实际提交整理；日期采用 Asia/Shanghai。v1.0.0、v1.1.0 只有源码标签，没有 GitHub Release；v1.2.0 同时标注原始标签日期与后来的 GitHub 发布日期。重写前的历史另保存在 `legacy/pre-rewrite-2026-08` 标签，不混入本版本线。

## v1.3.5 — 2026-11-12

- 修复 Windows 安装后桌面和开始菜单快捷方式显示白纸图标的问题，快捷方式现在显式使用应用图标。
- 修正客户端内置的更新签名公钥，使其与发布签名密钥一致。
- 仅提供 Windows x64 NSIS 安装包、对应 `.sig` 和 `latest.json`，是停止维护前的最后发布版本。

[Release 与下载](https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.3.5) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.3.4...v1.3.5)

## v1.3.4 — 2026-11-12

- 修复 Windows 更新清单缺失时的检查提示，改为安全回退到发布页，不绕过签名安装。
- 发布流程收敛为 Windows NSIS，并生成 Tauri 签名更新清单（`latest.json` 与 `.sig`）。
- 提升备份、搜索、相册进度、媒体缓存和更新包完整性校验的稳定性。
- 发布页文档与安全说明统一迁移到 GitHub，并明确后续 Release 仅提供 Windows 安装包。

### 数据与使用体验

- 备份递归收集账号子目录中的图片，恢复时校验数据库、路径和大小；失败时保留原有数据。
- 中文搜索使用子串匹配，并正确处理搜索词中的通配字符；归档列表与媒体时间轴按新到旧浏览。
- 修复本地媒体访问和相册照片另存，删除动态后清理对应缓存，视频缓存加入容量上限。
- 相册备份与离线导出的进度改为实时事件通知，降低持续轮询的开销。

仅提供 Windows x64 安装包和签名更新文件。此版本的签名公钥随后在 v1.3.5 修正，建议 Windows 用户选择 v1.3.5。

[Release 与下载](https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.3.4) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.3.3...v1.3.4)

## v1.3.3 — 2026-11-08

- 新增全局搜索，统一检索动态、评论、留言、相册、照片与好友昵称，结果可跳转到对应页面。
- 离线包导出显示阶段、数量与进度，支持取消和按目标文件增量导出；失败或取消时保留已有导出文件。
- 设置页新增 `.qzbak` 完整备份与恢复，包含数据库与图片；恢复前校验完整性，失败自动回滚。
- 深色模式保持用户选择，首次启动跟随系统主题；新增启动时检查更新和媒体按需加载开关。
- 修复 GLib 依赖安全问题及其他依赖漏洞，统一 Tauri 前后端依赖版本。

历史资产包含 Windows、macOS、Linux（AppImage / deb）、Android APK 和未签名 iOS IPA。

[Release 与下载](https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.3.3) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.3.2...v1.3.3)

## v1.3.2 — 2026-11-06

- 更新支持在应用内下载安装包，显示下载进度并支持取消；下载失败时尝试备用地址。
- Windows 新增安装并重启入口；macOS 打开下载的 DMG，Linux 打开安装包所在目录。
- 旧版用户需先安装具备应用内下载能力的版本；Windows 用户现建议直接使用最后发布的 v1.3.5。

这是历史下载更新机制；Windows 官方签名更新器在 v1.3.4 引入，不能将此前的更新流程视为签名更新。历史资产包含 Windows、macOS、Linux（AppImage / deb）、Android APK 和未签名 iOS IPA。

[Release 与下载](https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.3.2) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.3.1...v1.3.2)

## v1.3.1 — 2026-11-06

- “关于”页新增更新日志时间线，展示各版本主要变化并标记当前版本。
- 提供完整发布页入口，便于查看说明与下载文件。

历史资产包含 Windows、macOS、Linux（AppImage / deb）、Android APK 和未签名 iOS IPA。

[Release 与下载](https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.3.1) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.3.0...v1.3.1)

## v1.3.0 — 2026-11-06

- 新增 QQ 空间说说主页同步，与互动列表互补，可归档主页接口仍能返回的无互动说说；无法恢复已删除或无权访问的内容。
- 首页新增“那年今日”，按历史同日汇总动态；归档照片保留原始时间，方便本地按时间整理。
- 新增隐私保护模式，模糊头像、昵称、QQ 号和媒体预览；界面更新为玻璃卡片、网格与光效，适配深色模式。
- 桌面端视频封面支持悬停静音预览；数据库启用 WAL 模式，改善归档、相册备份和查询并发时的体验。
- 更新检查以 GitHub 为默认来源，并提供备用来源。

首次在 GitHub 提供 Windows、macOS、Linux（AppImage / deb）、Android APK 和未签名 iOS IPA。iOS 需自行签名侧载；macOS 未公证。

[Release 与下载](https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.3.0) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.2.0...v1.3.0)

## v1.2.0 — 2026-09-06

- 新增完整留言板同步，保存留言及回复。
- 新增好友列表同步与联系人浏览。
- 新增 JSON / Markdown 导出，包含评论、回复和点赞名单，便于进一步整理或阅读。

源码标签日期为 2026-09-06；GitHub 首次发布于 **2026-11-05**，仅附 Windows x64 安装包，没有其他平台安装包。

[Release 与下载](https://github.com/yuchen0x1/Qzone-Archive/releases/tag/v1.2.0) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.1.0...v1.2.0)

## v1.1.0 — 2026-09-06

- 新增相册备份，支持按相册批量下载原图、断点续传和失败照片重试。
- 新增 ZIP 离线包导出，将 HTML 页面、本地媒体和结构化 `data.json` 一起打包，方便离线浏览和迁移。

此版本只有源码标签，未创建 GitHub Release，也没有对应 GitHub 安装包。

[源码标签](https://github.com/yuchen0x1/Qzone-Archive/tree/v1.1.0) · [完整差异](https://github.com/yuchen0x1/Qzone-Archive/compare/v1.0.0...v1.1.0)

## v1.0.0 — 2026-09-05

- 当前重写版本线的初始版本，采用 Tauri 2、Vue 3 与本地 SQLite 存储。
- 提供扫码登录、互动列表归档、断点续传与频率保护，保存可访问的动态和互动记录。
- 支持本地归档浏览、联系人、媒体时光轴、HTML 导出和相册回收站恢复。

此版本只有源码标签，未创建 GitHub Release，也没有对应 GitHub 安装包。

[源码标签](https://github.com/yuchen0x1/Qzone-Archive/tree/v1.0.0)

## 存档说明 — 2026-11-13

- 停止功能开发、依赖更新与安全修复，不再承诺 Issue、PR 或漏洞报告的响应时间。
- 保留源码、许可证、版本标签与历史 Release 下载；QQ 平台接口变化后的兼容性不再维护。
- 更新日志补全至 v1.0.0–v1.3.5；本次源码与文档整理不会改变已经发布的安装包。
- Windows 安装包没有 Authenticode 代码签名；Tauri 更新签名不能代替 Windows 代码签名，也不代表完成了真实跨版本安装验证。
