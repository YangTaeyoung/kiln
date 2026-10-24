# Kiln docs — naturalness pass

Goal: the page must read as if a native technical writer at a good product company wrote it in this language, not as a translation of English.

## Change
- Translationese: literal word order, stacked nouns, "~하는 것은 ~입니다" chains, overuse of pronouns (당신/あなた/你), passive voice where active is natural, English punctuation habits.
- Sentences that are correct but stiff. Prefer short, direct sentences and the verbs a developer would actually use.
- Inconsistent terms across pages. Keep one term per concept (e.g. ko: 워크스페이스, 작업, 패널, 분할 화면, 세션, 에이전트; ja: ワークスペース, 作業, パネル, 分割, セッション, エージェント; zh-CN: 工作区, 任务, 面板, 分屏, 会话, 代理).
- Headings: make them short and scannable, in the language's natural docs style (ko: 명사형 또는 "~하기"; ja: 体言止め or 「〜する」; zh: 简短动宾或名词短语).

## Keep exactly
- Facts. Do not add, drop or soften any statement.
- Front matter keys, every `{#id}`, link targets, code, inline code, commands, paths, keys, product names, table structure and callout structure.
- Bold UI labels: keep the app's label for that language (from `crates/kiln-common/locales/messages.json`). Do not rephrase a UI label.

## Style per language
- ko: 합니다체. 문장 끝을 다양하게 하되 과한 존댓말("~해 주시기 바랍니다") 대신 "~하세요". 영어 괄호 병기는 처음 한 번만, 꼭 필요할 때만.
- ja: です・ます調。全角と半角の間に半角スペース。カタカナ語の過多を避け、自然な技術文書の語彙を使う。
- zh-CN: 简洁、直接的技术文档语气；中英文之间加空格；避免"你可以……"开头的堆叠。

Rewrite the file in place. After each file, check that the `{#id}` set, link targets and code blocks are identical to the English source in `docs-src/en/`.
