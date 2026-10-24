# Kiln product docs — writing brief

Kiln is positioned as a **terminal-first IDE for agent work** (native app, Rust + egui). Never describe it as a CLI tool.
The `kiln` command exists, but mention it only where a user genuinely needs it (e.g. `kiln activity`, `kiln notify`).

## Source of truth
Only facts from this repository at the commit being documented (normally `origin/main`).
Read files with `git show origin/main:<path>` so a stale checkout is never the source.
Main sources: README.md and docs/*.md (not docs/maintainers, docs/plans, docs/superpowers, docs/releases, docs/prototypes, docs/development.md, docs/architecture.md except for user-relevant concepts).
If a fact is not in those files, leave it out. No invented shortcuts, menu names, numbers, or features. Do not promise future work.
The documented version is recorded in `site/_build/source.json`. Platforms: macOS 11+ on Apple Silicon (notarized, Sparkle updates); Windows 11 x64/ARM64 (not code-signed yet; SmartScreen may prompt).

## Audience and voice
End users: developers using the app. Exclude maintainer, test, fixture, CI and verification details, env vars meant for tests, source file paths, and upstream version-check notes.
Write like a polished product site (Linear/Raycast docs): task-oriented, second person, short paragraphs, plain words, active voice.
Name UI exactly as the app shows it in English (bold): **Settings → Accounts**, **New task**, **Link conversations**.
No em-dash asides, no "not X but Y", no marketing fluff. Each page opens with a 1–2 sentence lead.

## File format (Markdown, Python-Markdown with tables + fenced code)
```
---
title: Workspaces and tasks
description: One-sentence summary for the page header and search.
---

Lead paragraph (1–2 sentences).

## Section
Text, lists, tables, ```sh code```.

> **Note:** for important caveats (also **Tip:**, **Warning:**).
```
- H2 for sections, H3 for subsections. No H1 (the title comes from front matter).
- Keyboard keys in backticks like `⌘N`.
- Links to other pages: `[Accounts](accounts.html)` using the page slugs below; anchors `accounts.html#antigravity-accounts` only for H2/H3 ids (slugified lowercase, hyphens).
- Length: 400–1100 words per page depending on the topic. Prefer tables for option/behavior lists.

## Pages (slug — title — scope)
1. index — Introduction — what Kiln is; core concepts (workspace, task, panel/split, inspector tools, session daemon, notifications); where to go next.
2. install — Install and update — macOS + Windows install (script and download), requirements, first launch, updates, external tools (gh, agents), language setting.
3. workspaces — Workspaces and tasks — named workspaces, parent folders with several repos, task list ordering/states, splits and pane layout, drafts, switching, closing.
4. terminal — Terminal — shell integration, activity/notifications (`kiln activity`, `kiln notify` as the documented integration), command history, shell completion, connection recovery, sessions that survive closing the app, local network permission if user-relevant, Windows shells.
5. agents — Agents — Claude Code, Codex, Antigravity: starting tasks, agent request + repository context, indicators/colors, running/waiting/done/failed, Codex capacity recovery.
6. accounts — Accounts — Claude Code/Codex profiles, browser sign-in, apply/switch, sign in again, usage refresh, Antigravity accounts on macOS.
7. conversations — Conversations and updates — conversation sync (Claude Code ↔ Codex, Antigravity → both), status dialog, /resume, CLI updates, restart in the same conversation.
8. git — Git and GitHub — history, squash, cherry-pick, drop, edit message, diffs, pull requests/issues, repository selection in multi-repo workspaces.
9. editor — Editor and search — file editing, language servers, search, problems.
10. databases — Databases — connections, SQL console, completion, query execution, editing SELECT results, tables/schema.
11. remote — Remote files and SSH — S3/OCI/GCS/R2 profiles, FTP/FTPS/SFTP, SSH config hosts, SSH terminals, credential storage.
