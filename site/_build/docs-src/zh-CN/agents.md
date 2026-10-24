---
title: 代理
description: 在工作区中启动 Claude Code、Codex 和 Antigravity 任务，跟踪任务状态，并在服务器过载时由 Kiln 重试 Codex 回合。
---

Kiln 在工作区面板中运行 Claude Code、Codex 和 Google 的 Antigravity CLI（`agy`）。写好请求、选好代理，每个任务的状态都能在侧边栏看到，不必在一个个终端之间翻找。

## 开始之前 {#before-you-start}

这些代理都是外部工具。请为要用的每个代理安装官方 CLI 并登录。Kiln 不含其中任何一个的订阅。

- **Claude Code** 和 **Codex**：安装 CLI，然后在**设置 → 账户**中添加账户。详见[账户](accounts.html)。
- **Antigravity**：用 Google 提供的安装程序安装 `agy`。在 macOS 上，从**设置 → 账户 → Antigravity** 登录；在 Windows 上，在终端中运行 `agy` 登录。Kiln 会在 `PATH`、`~/.local/bin`、`/opt/homebrew/bin` 和 `/usr/local/bin` 中查找 `agy`。

在 Windows 上，Claude Code 和 Codex 以原生 Windows 程序运行，主目录分别是 `%USERPROFILE%\.codex` 和 `%USERPROFILE%\.claude`。

## 启动任务 {#start-a-task}

选择**新建任务**，输入请求，选择代理并启动。代理选择器（Claude Code、Codex 或 Antigravity）固定在请求输入框正下方。仓库上下文单独滚动，摘要再长也不会把启动按钮挤出视野。

Antigravity 还有以下入口：

| 操作 | 作用 |
| --- | --- |
| **新建任务 → Antigravity** | 在当前工作区中启动 `agy`。 |
| **新建任务 → 在文件夹中打开 → Antigravity** | 先选择文件夹，再在该文件夹中启动 `agy`。 |
| **新建 Antigravity 任务**（命令面板，`⌘K`） | 启动一个 Antigravity 任务。 |

在请求下方选择 Antigravity 时，Kiln 会把请求和仓库摘要作为初始提示词传给 `agy`。`antigravity` 命令打开的是 Antigravity 编辑器，因此 Kiln 不把它当作代理。

未发送的请求随工作区一起保存，Kiln 重新打开时会恢复。关闭工作区时，丢弃这些请求前会先询问。

## 跨多个仓库工作 {#work-across-several-repositories}

如果前端、后端和共享库分属不同的 Git 仓库，就打开它们的父文件夹，例如 `~/dev/projects`。Kiln 把这个文件夹作为代理的工作根目录，还可以把扫描到的仓库结构随请求一起附上，一个请求就能覆盖所有仓库。

在 Git 或 GitHub 工具中查看某个仓库，不会改变任务的工作目录。要让 Git 操作针对某个仓库，请使用这些工具中的仓库选择器。详见[工作区与任务](workspaces.html)和 [Git 与 GitHub](git.html)。

## 跟踪任务状态 {#follow-task-state}

侧边栏中的每一行任务都显示其代理上报的状态。

| 状态 | 含义 |
| --- | --- |
| 处理中 | 代理正在工作。 |
| 等待输入 | 代理需要你输入。 |
| 已完成 | 代理已完成。 |
| 失败 | 代理上报失败。 |
| 会话已打开 | 没有上报任何状态。Kiln 只把会话显示为打开，不做猜测。 |

运行中的工作排在最前，其次是等待输入的请求，再往后是较早的失败。点击任务即可跳到它所在的工作区、标签页和拆分视图，即使该面板因缩放而被隐藏。

### Kiln 如何获知状态 {#how-kiln-knows-the-state}

代理必须上报自己的状态，或者显示 Kiln 能识别的运行中标题，Kiln 才能准确显示状态。

- Kiln 能识别 Claude Code 和 Codex 终端标题中的活动标记，无需额外设置就能显示运行中的活动。
- Claude Code 的 `✳` 标记不算作完成，因为对话框打开时它也会出现。
- 显式的等待和失败信号优先于标题标记。
- 空闲的终端绝不会被当作已完成的任务；启动命令尚未结束，也不会让空闲的代理显示为运行中。

脚本和其他工具可以用 `kiln activity running`、`waiting`、`done` 或 `failed` 上报状态。活动上报和通知详见[终端](terminal.html)。

### 指示器颜色 {#indicator-colors}

| 代理 | 指示器 |
| --- | --- |
| Claude Code | 橙色 |
| Codex | 蓝色 |
| Antigravity | 标签页、任务列表和侧边栏中显示 Antigravity 标志 |
| 无法识别的会话 | 中性色 |

指示器出现在侧边栏、标签页和面板标题栏中。在有拆分视图的标签页里，各状态优先级相同时，以获得焦点的面板为准。警告始终优先。

## 通知与用量限制 {#notifications-and-usage-limits}

通知中心保留最近的通知，每条通知都关联到对应的会话。读过通知后，该会话的待处理状态随即清除。在 Windows 上，代理通知以 Windows 弹窗通知的形式出现。

Antigravity 上报配额已用完时，Kiln 会显示一条用量限制通知，并注明重置时间。对于 Claude Code 和 Codex，面板始终使用打开时的账户。某个账户达到用量限制后，请在**设置 → 账户**中切换到其他账户，再新开一个面板。详见[账户](accounts.html#choose-which-account-to-use)。

## Codex 容量恢复 {#codex-capacity-recovery}

Kiln 打开期间，如果 Codex 因服务器过载而结束了某个回合，由 Kiln 管理的 Codex 面板会重试该回合。只有这一种 Codex 错误会触发重试。用量限制、登录失败、流错误、已取消的回合和成功的回合都不会处理。

- 重试间隔依次为 15、30、60、120 和 240 秒。面板上会显示倒计时和**取消自动重试**按钮。
- 失败后在面板中输入任何内容，都会取消待执行的重试，输入到一半的提示词也算。
- 你自己提交新请求时，会重新开始一轮重试。
- 重试沿用同一个账户和模型。

如果 Codex CLI 仍然开着，Kiln 会在同一对话中发送一条继续提示词。如果 CLI 已经退出，Kiln 会用原来的账户和工作目录恢复同一对话，再发送这条提示词。提示词会要求 Codex 先检查当前状态，再决定是否重复已完成的操作。它不会重放你的原始请求，失败回合中做出的更改也不会回滚。

重试只适用于用已保存账户打开、且 Kiln 已确认其对话的 Codex 面板。Kiln 开始监视运行中的 CLI 之前发生的失败，需要你自行处理。

## Codex 后台服务器设置 {#codex-background-server-settings}

Kiln 新开的 Codex 面板以 `--no-daemon` 运行，与 Codex 自己的共享后台服务器相互独立。如果某个 Codex 会话提示 **Background server has incompatible feature settings**，说明该会话的功能标志与服务器保存的不一致。可以选择：

| 选项 | 效果 |
| --- | --- |
| **Run without daemon this time** | 不重启共享的 Codex 服务器，直接启动本次会话。对应的 CLI 命令是 `codex --no-daemon`。 |
| **Restart with these settings** | 更改共享设置。这可能会中断其他客户端正在进行或排队的工作，请先确认。 |

## 后续步骤 {#next-steps}

- [账户](accounts.html)：登录、切换账户和查看用量。
- [对话与更新](conversations.html)：在另一个 CLI 中继续对话，更新后重启。
