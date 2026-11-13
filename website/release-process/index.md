---
title: 存档与历史发布
---

# 存档与历史发布

项目于 **2026-11-13** 停止维护并转为存档，最后发布版本为 **v1.3.5**。不再计划发布新版本、修复依赖或适配 QQ 空间接口变化。

## 保留的内容

- [全部发布记录](https://github.com/yuchen0x1/Qzone-Archive/blob/main/docs/releases/release-notes.md)：当前版本线的 v1.0.0–v1.3.5。
- [GitHub Releases](https://github.com/yuchen0x1/Qzone-Archive/releases)：7 个已有 Release 及对应安装包；v1.0.0、v1.1.0 只有源码标签。
- v1.3.5 提供 Windows x64 NSIS 安装包、`.sig` 与 `latest.json`；v1.3.0–v1.3.3 的跨平台安装包继续保留。
- 源码、许可证、构建配置与历史标签保留，便于按 GPLv3 研究或继续维护自己的 Fork。

## 历史发布机制

`.github/workflows/release.yml` 原用于根据语义化版本标签构建 Windows NSIS 并上传 Tauri 更新签名；`release-assets.yml` 原用于手动修复已有 Windows Release。存档后不再执行这些流程。

旧 Gitee 流水线已经移除。原有 Windows 更新机制及验证限制见 [自动更新说明](https://github.com/yuchen0x1/Qzone-Archive/blob/main/docs/AUTO_UPDATE.md)。

存档整理只修改源码和文档，不重建历史安装包，也不新增 Release。
