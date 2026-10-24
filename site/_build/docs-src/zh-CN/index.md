---
title: 简介
description: Kiln 是面向代理工作、以终端为核心的 IDE，终端、AI 代理、Git、文件和数据库都在同一个原生工作区里。
---

Kiln 是面向代理工作、以终端为核心的 IDE。终端、AI 代理、Git、文件和数据库集中在一个 macOS 和 Windows 原生应用里，整个任务始终在眼前。

打开单个仓库，或者打开一个同时包含前端、后端和共享库的父文件夹。在终端面板里运行 Claude Code、Codex 或 Antigravity，随时看到哪些任务在等你处理，不离开窗口就能审阅结果。

## 核心概念 {#core-concepts}

### 工作区 {#workspace}

工作区是文件夹的一个命名视图，各自保存任务、终端会话、拆分布局和尚未发送的请求。同一个文件夹可以有多个工作区，比如 **API 审查**和**文档**；Kiln 重新打开时会分别恢复每个工作区。文件和 Git 历史属于文件夹，因此同一文件夹下的所有工作区共用它们。

这个文件夹可以是一个仓库，也可以是包含多个仓库的父目录。代理在这个共同的根目录下工作，Git 操作则针对你选中的仓库。详见[工作区与任务](workspaces.html)。

### 任务 {#task}

任务就是工作区终端里正在进行的工作：可以是你直接使用的 Shell，也可以是通过**新建任务**启动的代理。侧边栏按工作区列出任务，并显示它们上报的状态：

| 状态 | 含义 |
| --- | --- |
| 处理中 | 代理或命令上报正在工作。 |
| 等待输入 | 代理在等你回复。 |
| 已完成 | 代理上报已经完成。 |
| 失败 | 代理或命令上报失败。 |

会话没有上报状态时，Kiln 只把它显示为打开的会话，不会根据终端输出去猜。空闲的终端不算已完成的任务。点击任务，即可直接跳到它所在的工作区、标签页和拆分视图。

### 面板与拆分视图 {#panels-and-splits}

每个终端都在一个面板里。面板标题栏上有向右拆分、向下拆分和关闭按钮。把面板标题拖到另一个面板上即可调整布局，移动后终端会话和正在运行的进程都保持不变。详见[拆分与面板布局](workspaces.html#splits-and-pane-layout)。

### 检查器与工具 {#inspector-and-tools}

右侧的工作区检查器包含**文件**、**更改**、**GitHub**，以及一个数据库图标，用于管理连接、表和 SQL 控制台。标题栏的**工具**菜单汇总了文件、搜索、源代码管理、GitHub、数据库、远程连接和问题，每一项都会在当前工作旁边打开。窗口较窄时，点击**返回任务**回到终端。

### 会话守护进程 {#session-daemon}

终端由一个独立的本地守护进程托管。关闭 Kiln 窗口后，其中的 Shell 和代理继续运行，重新打开 Kiln 就能回到它们。更新时，正在运行的终端也不会中断。系统重启或手动停止守护进程则不同于关闭窗口，可能会结束会话。详见[关闭窗口后保留的会话](terminal.html#sessions-that-outlive-the-window)。

### 通知 {#notifications}

代理和脚本可以通过本地终端转义序列，或者 `kiln activity` 和 `kiln notify` 命令上报活动、发送通知。这些方式不会把你的提示词发送给其他服务。通知中心保留最近的通知，每条通知都关联到对应的会话；读过通知后，该会话的待处理状态随即清除。在 Windows 上，代理通知还会以 Windows 弹窗通知的形式出现。详见[终端](terminal.html#activity-and-notifications)。

## 准备工作 {#what-you-need}

- Apple Silicon 上的 macOS 11 或更高版本，或者 x64 / ARM64 上的 Windows 11。
- 你打算使用的代理 CLI：Claude Code、Codex 或 Antigravity（`agy`），以及用于 GitHub 的 `gh`。Kiln 不含任何代理的订阅，请自行安装并登录要用的工具。

应用界面支持英语、韩语、日语和简体中文。

## 下一步 {#where-to-go-next}

| 页面 | 内容 |
| --- | --- |
| [安装与更新](install.html) | 在 macOS 或 Windows 上安装、首次启动、更新和语言 |
| [工作区与任务](workspaces.html) | 命名工作区、多仓库文件夹、任务列表、拆分视图和草稿 |
| [终端](terminal.html) | Shell 集成、活动、命令历史、补全和连接恢复 |
| [代理](agents.html) | 启动 Claude Code、Codex 和 Antigravity 任务，查看任务状态 |
| [账户](accounts.html) | 代理配置、浏览器登录和账户切换 |
| [对话与更新](conversations.html) | 代理之间的对话同步和 CLI 更新 |
| [Git 与 GitHub](git.html) | 历史、压缩、拣选、diff 和拉取请求 |
| [编辑器与搜索](editor.html) | 文件编辑、语言服务器、搜索和问题 |
| [数据库](databases.html) | 连接、SQL 控制台和表编辑 |
| [远程文件与 SSH](remote.html) | 对象存储、FTP/SFTP 和 SSH 终端 |
