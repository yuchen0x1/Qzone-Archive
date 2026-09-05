---
title: 发布流程
---

# 发布流程

1. 在功能分支完成一个可审查的变更。
2. 运行应用和文档站各自的构建检查；推送至 `main` 时，Gitee 流水线（`.workflow/main-pipeline.yml`）会自动执行前端与文档站构建，Rust 侧检查（`cargo fmt --check`、`cargo test`）目前由维护者在本地执行。
3. 提交使用 Conventional Commits，例如 `docs: add installation guide`。
4. 合并至 `main` 后，在 Gitee Pages 中同步发布 `website/` 文档内容。
5. 发布新版本时：将版本号统一更新（`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`nix/qzonearchive.nix` 及对应锁文件），推送 `v*` 标签，并在 Gitee Releases 中创建发布、上传 Windows、macOS、Linux 与移动端安装包。

文档部署只使用 `main` 分支的 `website/` 内容。开发或预览中的文档分支不会覆盖公开站点。

Linux 发布产物包括：

- `.deb`：Debian / Ubuntu
- `.rpm`：Fedora / openSUSE / RHEL 系
- `.AppImage`：通用 Linux 包

本地制作 Linux 包时，需要先安装 Linux 系统依赖，再执行：

```bash
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  build-essential \
  curl \
  wget \
  file \
  libxdo-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
npm ci
npx tauri build --bundles appimage,deb,rpm
```
