# Kiln docs — translation brief

Translate every English page in `docs-src/en/*.md` into the target language, writing `docs-src/<lang>/<same-name>.md`.

## Keep exactly
- Front matter keys (`title`, `description`) — translate the values.
- Every heading's explicit id suffix, e.g. `## Antigravity accounts {#antigravity-accounts}` → `## Antigravity 계정 {#antigravity-accounts}`. Never change or drop `{#...}`.
- Link targets (`accounts.html#...`), code blocks, inline code, commands, file paths, keyboard keys, product names (Kiln, Claude Code, Codex, Antigravity, agy, GitHub, PostgreSQL…), and Markdown structure (lists, tables, callouts).
- Callout labels: translate `**Note:**` / `**Tip:**` / `**Warning:**` to the target language's label (ko: 참고/팁/주의, ja: 注意/ヒント/警告, zh-CN: 注意/提示/警告) and keep them bold followed by a colon.

## UI labels
Kiln's app itself is localized. When the English text names a UI element in bold (e.g. **Settings → Accounts**, **New task**, **Link conversations**), use the label the app shows in the target language. Look it up in the app's message catalog:
`crates/kiln-common/locales/messages.json`
Keys are Korean source strings; each entry has `en`, `ja`, `zh-CN`. Search by the English value to find the matching translation (for Korean use the key itself). If a label is not in the catalog, translate it naturally and keep the English in parentheses the first time, e.g. **설정 → 계정 (Settings → Accounts)**.

## Voice
- Natural product-docs prose for native developers, not a literal translation. Short sentences, active voice.
- ko: 합니다체, 개발자에게 익숙한 용어는 원어 유지(commit, diff, worktree 등은 문맥에 맞게 "커밋", "diff", "worktree").
- ja: です・ます調。全角と半角の間にスペースを入れる慣習に合わせる (例: Kiln の設定)。
- zh-CN: 简洁的技术文档语气，中英文之间加空格。
- Do not add or remove facts.
