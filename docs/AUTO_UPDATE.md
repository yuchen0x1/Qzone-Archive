# 自动更新说明

空间归档的 Windows 更新使用 Tauri 官方 `tauri-plugin-updater`。更新清单、Windows NSIS 安装包和签名文件由 GitHub Releases 提供，客户端内置公钥并在安装前验证签名。后续 Release 不再上传 macOS、Linux、Android 或 iOS 构建产物；历史 Release 可能仍保留这些资产。

## 客户端流程

1. Windows 启动后或用户在“关于”页手动检查时，请求配置的 `latest.json`。
2. Tauri updater 使用 `src-tauri/tauri.conf.json` 中的 HTTPS endpoint 和公钥验证版本清单及安装包签名。
3. 用户确认后，下载回调提供真实字节进度；下载完成后才允许安装。
4. Windows 安装器安装后自动重启。
5. 签名、网络或安装失败时，当前版本保持可用，不执行未经验证的安装。

如果旧 Release 尚未上传 `latest.json`，客户端会回退到 GitHub Release API 读取版本，但只提供发布页入口，不会把未签名安装包当作应用内更新。

## 发布前配置

在 GitHub 仓库的 Actions secrets 中配置：

- `TAURI_SIGNING_PRIVATE_KEY`：更新签名私钥的完整内容。
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：私钥密码；无密码密钥可为空，但生产环境建议改用密码保护密钥。

私钥只能保存在本机安全目录或 GitHub Secrets，不能提交到仓库、前端代码、日志或 Release 资产。当前项目公钥位于 Tauri 配置中；私钥不在项目目录内。

## 发布步骤

1. 在 `main` 上完成修改、验证并提交 Conventional Commit。
2. 从 `main` 创建并推送语义化版本标签，例如 `v1.3.4`。
3. `.github/workflows/release.yml` 在 Windows 上构建 NSIS 安装包，并上传 `.sig` 文件和 `latest.json`。
4. 工作流会直接发布 Release；确认 GitHub Actions 成功后，客户端即可读取更新清单。
5. 发布后，客户端通过 `https://github.com/yuchen0x1/Qzone-Archive/releases/latest/download/latest.json` 获取更新。

工作流只在 `vMAJOR.MINOR.PATCH` 标签触发；没有推送标签不会创建 Release。需要修复历史 Release 时，可手动运行 `Repair Windows Release`，默认从 `main` 的签名更新配置重新生成并上传 Windows NSIS、`.sig` 和 `latest.json`；如果目标标签对应的源码已经包含更新配置，也可以把 `source_ref` 改为该标签。

## 本地验证

```powershell
npm ci
npm run build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

没有真实 GitHub Secrets 和已发布 Release 时，只能验证本地编译、配置和更新器单元测试，不能把签名发布或跨版本安装写成已通过。真实升级测试应使用测试标签，确认签名失败、损坏下载、网络中断和用户取消时旧版本仍可启动。

## 故障恢复与限制

- 更新 endpoint 不可访问时，应用继续运行，用户可从关于页打开发布页。
- 签名校验失败时，官方 updater 拒绝安装；不要手工绕过校验。
- Windows 代码签名证书尚未配置，首次安装可能显示 SmartScreen 警告；这与 Tauri 更新签名是两套密钥。
- 历史 Release 可能仍保留旧的跨平台资产；新工作流不会再上传这些资产。GitHub 自动生成的 Source code 压缩包仍会显示。
- `releases/latest` 不会返回草稿或预发布版本；工作流发布完成前客户端不会看到新版本。
