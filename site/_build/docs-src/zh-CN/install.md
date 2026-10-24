---
title: 安装与更新
description: 在 macOS 或 Windows 上安装 Kiln，打开第一个工作区，让应用保持最新，并选择界面语言。
---

一条命令即可安装 Kiln，也可以从 GitHub Releases 下载。安装后 Kiln 会自动更新，更新时不会打扰正在运行的终端。

网站会优先显示浏览器报告的操作系统对应的下载按钮，另一平台仍可选择。移动设备或无法识别的系统会同时显示两个选项。点击按钮后，可在 GitHub Releases 中选择适合电脑的压缩包。

## 系统要求 {#requirements}

| 平台 | 要求 |
| --- | --- |
| macOS | 搭载 Apple Silicon 的 Mac，macOS 11 或更高版本。构建已通过公证。 |
| Windows | x64 或 ARM64 上的 Windows 11。构建暂未进行代码签名。 |

目前不提供 Intel Mac 和 Linux 构建。Windows 10 未经测试。

## 在 macOS 上安装 {#install-on-macos}

### 使用安装脚本 {#use-the-install-script}

在终端中运行：

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | sh
```

脚本会下载最新的 Apple Silicon 稳定版，先校验发布校验和、Developer ID 签名、应用包标识和 Apple 公证，再安装到 `/Applications`。如果已经装有 Kiln，脚本不会动现有安装；升级请改用 **Kiln → 检查更新**。

要安装到其他位置，设置 `KILN_INSTALL_DIR`：

```sh
curl -fsSL https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.sh | KILN_INSTALL_DIR="$HOME/Applications" sh
```

### 下载应用 {#download-the-app}

1. 从 [GitHub Releases](https://github.com/YangTaeyoung/kiln/releases/latest) 下载 Apple Silicon 压缩包。
2. 解压。
3. 先把 **Kiln.app** 移到**应用程序**文件夹，再打开。

## 在 Windows 上安装 {#install-on-windows}

### 使用 PowerShell 脚本 {#use-the-powershell-script}

在 PowerShell 中运行：

```powershell
irm https://raw.githubusercontent.com/YangTaeyoung/kiln/main/install.ps1 | iex
```

脚本会按设备架构选择 x64 或 ARM64 压缩包，校验发布校验和以及压缩包内的路径和版本，然后：

- 把 Kiln 安装到 `%LOCALAPPDATA%\Programs\Kiln`；
- 在“开始”菜单中添加 **Kiln** 快捷方式；
- 把 `bin` 文件夹加入用户 `PATH`，以便使用 `kiln` 命令。

要安装到其他文件夹，设置 `KILN_INSTALL_DIR`。已有的安装会原样保留。

### 下载压缩包 {#download-the-archive}

也可以从 [GitHub Releases](https://github.com/YangTaeyoung/kiln/releases/latest) 下载与你的架构对应的压缩包。

> **注意：** Windows 构建暂未进行代码签名。通过安装脚本下载不会触发 SmartScreen，用浏览器下载的压缩包则可能触发。只有来自 Releases 页面的压缩包，才应选择**更多信息 → 仍要运行**。

## 首次启动 {#first-launch}

1. 从“应用程序”或“开始”菜单打开 **Kiln**。
2. 点击工作区侧边栏中的 **+** 按钮（macOS 上为 `⌘N`）创建工作区，为它命名并选择项目文件夹。
3. 直接使用终端，或者选择**新建任务**，输入请求，选好代理后启动。

这个文件夹可以是单个仓库，也可以是包含多个仓库的父文件夹。工作区、任务和拆分视图如何配合，详见[工作区与任务](workspaces.html)。

在 Windows 上，如果 Kiln 已在运行，再次启动会把现有窗口切到前台，并在其中打开你指定的文件夹。

## 安装所需工具 {#install-the-tools-you-use}

Kiln 调用的是你已安装的代理和 GitHub 命令行工具，既不内置这些工具，也不含订阅。请安装并登录你要用的工具：

| 工具 | 用途 |
| --- | --- |
| Claude Code（`claude`） | Claude 任务 |
| Codex（`codex`） | Codex 任务 |
| Antigravity（`agy`） | Antigravity 任务。请用 Google 提供的安装程序安装。 |
| `gh` | GitHub 功能 |

Kiln 会在 `PATH`、`~/.local/bin`、`/opt/homebrew/bin` 和 `/usr/local/bin` 中查找 `agy`。在 Windows 上，Claude Code 和 Codex 以原生 Windows 程序运行，数据保存在 `%USERPROFILE%\.codex` 和 `%USERPROFILE%\.claude`。

Claude Code 和 Codex 账户在**设置 → 账户**中登录；macOS 上的 Antigravity 在**设置 → 账户 → Antigravity** 中登录。在 Windows 上，请在终端中运行 `agy` 登录 Antigravity。详见[账户](accounts.html)。要查看已安装的 CLI 版本或更新它们，打开**设置 → 账户 → CLI 更新**，详见[对话与更新](conversations.html)。

## 更新 Kiln {#update-kiln}

更新不会中断正在运行的终端会话。

### macOS {#macos}

Kiln 会自动检查更新，并通过 Sparkle 更新对话框安装。要手动检查，选择 **Kiln → 检查更新**，或者使用**设置 → 关于**中的更新控件。自动检查可以在设置中关闭。

### Windows {#windows}

Kiln 每天检查一次更新。要手动检查，打开**设置 → 关于 → 检查更新**，然后选择**安装并重新启动**。替换任何文件之前，Kiln 会先验证下载的压缩包与发布签名密钥是否匹配。正在运行的终端不受影响。

> **提示：**在 macOS 15 及更高版本上，如果某条访问本地网络的命令在更新前就已打开的面板中失败，请新开一个终端面板再试。详见 [macOS 上的本地网络访问](terminal.html#local-network-access-on-macos)。

## 选择语言 {#choose-a-language}

打开**设置 → 外观 → 语言**，选择英语、韩语、日语或简体中文。更改立即生效，下次启动时依然保留。

这只会改变 Kiln 自身的界面。终端输出、文件名、代码、提交信息和代理请求都保持原样。从早期版本升级上来的安装会继续使用韩语界面，直到你选择语言为止。

## 后续步骤 {#next-steps}

- [工作区与任务](workspaces.html)：整理文件夹和任务。
- [代理](agents.html)：启动第一个代理任务。
- [终端](terminal.html)：设置 Shell 集成和活动上报。
