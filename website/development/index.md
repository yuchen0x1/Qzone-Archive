---
title: 开发
---

# 开发

本项目已于 2026-11-13 停止维护并转为存档。以下命令供源码研究和 Fork 维护参考，原仓库不再接受常规 PR。

## 前置条件

- Node.js 20+
- Rust 1.77+
- Windows 开发环境需 WebView2
- Android 开发还需要 Android Studio、SDK 与 NDK

```bash
npm ci
npm run tauri dev
```

## 文档站

文档站是独立工作区，不会参与 Tauri 打包：

```bash
cd website
npm ci
npm run dev
```

文档内容使用 Markdown，放在 `website/` 下的目录索引页。提交前执行 `npm run build`。

## 提交与 Pull Request

历史开发约定见仓库的 [贡献指南](https://github.com/yuchen0x1/Qzone-Archive/blob/main/CONTRIBUTING.md)。继续开发请在自己的 Fork 中进行，并保留 GPLv3 许可和署名。
