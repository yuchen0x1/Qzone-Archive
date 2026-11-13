---
title: 安装
---

# 安装

项目已于 2026-11-13 停止维护并转为存档。从 [GitHub Releases](https://github.com/yuchen0x1/Qzone-Archive/releases) 下载最后发布的 v1.3.5 Windows `.exe` 安装程序、签名文件和 `latest.json`。v1.3.0–v1.3.3 的跨平台历史安装包仍保留，不再生成新包。

网站打开下载区域时会读取公开 Release API，显示当前线上版本和 Windows 安装包；网络不可达时仍可直接打开 Release 页面。桌面应用使用 Tauri 官方签名更新器，启动时或在“关于”页手动检查，只有签名校验通过后才下载并安装。

## Windows

下载 `.exe` 安装程序并完成安装。系统需要 Windows 10 或更新版本，通常已包含 WebView2。

## macOS

当前不随 Release 发布安装包，请参考开发文档从源码构建。首次启动时，系统可能要求在“隐私与安全性”中确认打开来源未知的应用。

## Android

当前不随 Release 发布 APK，请参考开发文档从源码构建。

## iOS

当前不随 Release 发布 IPA，请参考开发文档从源码构建并自行签名。IPA 不能直接安装到 iPhone，也不能直接上传 App Store。

## Linux

当前不随 Release 发布 Linux 安装包；以下命令仅适用于从源码或历史 Release 获取对应文件后的本地安装：

- `.deb`：适合 Debian、Ubuntu 及其衍生发行版
- `.rpm`：适合 Fedora、openSUSE、RHEL 系发行版
- `.AppImage`：适合大多数桌面发行版，包括无法直接使用 `.deb`/`.rpm` 的发行版

### Debian / Ubuntu

```bash
sudo apt install ./*.deb
```

也可以使用 `dpkg`：

```bash
sudo dpkg -i *.deb
```

> **`apt` 与 `dpkg` 的区别**：
> - `apt install`：会自动联网补齐依赖，日常推荐使用；
> - `dpkg -i`：只安装包本身，不处理依赖。如果提示缺少依赖，先执行下面的命令补装：
>
> ```bash
> sudo apt-get install -f
> ```

卸载：

```bash
sudo apt remove qzonearchive
```

> 如果提示找不到包，可用 `dpkg -l | grep qzonearchive` 确认实际安装的包名（一般为 `qzonearchive`）。

### Fedora / openSUSE / RHEL 系

```bash
sudo rpm -i *.rpm
```

卸载：

```bash
sudo rpm -e qzonearchive
```

> 如果提示找不到包，可用 `rpm -qa | grep qzonearchive` 确认实际安装的包名（一般为 `qzonearchive`）。

### 通用 AppImage（适用于其他 Linux 发行版）

```bash
chmod +x *.AppImage
./*.AppImage
```

> 如果提示 `libfuse.so.2` 相关错误（常见于 Ubuntu 24.04 及以上），先安装 FUSE 依赖，或改用免 FUSE 的解包运行方式：
> ```bash
> sudo apt install libfuse2   # Debian / Ubuntu
> ./*.AppImage --appimage-extract-and-run
> ```

AppImage 是免安装的绿色软件，删除文件即可完成卸载。如果桌面环境没有自动集成应用菜单，可以自行创建 `.desktop` 文件，也可以直接把 AppImage 放到本地路径手动启动。

### NixOS

当前 GitHub 历史 Release 没有附带 NixOS closure 安装包。仓库保留源码构建用的 Nix Flake；其版本和依赖哈希仍为旧状态，未在存档整理中验证，使用前需自行更新。下面仅保留构建命令供参考：

```bash
nix build
./result/bin/qzonearchive
```

也可以安装到当前用户 profile：

```bash
nix profile install .#qzonearchive
```

Linux 用户安装 QQ 客户端时，请按你自己发行版的要求选择 QQ 官方提供的 deb、rpm 或 Flatpak 版本；空间归档本身不绑定或内置 QQ 客户端，只需要登录后扫描 QQ 空间的二维码即可使用。

从源码构建 Linux 版本可参考[开发](../development/)。
