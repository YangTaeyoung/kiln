# Agent slash commands

Kiln combines the provider's published command names and aliases with the running
session's command and skill catalogs. Shortcut aliases resolve to the same GUI
action where available. A CLI command fallback retains the spelling and
arguments the user typed.

| Provider | Reference checked | Catalog |
| --- | --- | --- |
| Codex | 0.171.0 [tagged command definitions](https://github.com/openai/codex/blob/rust-v0.171.0/codex-rs/tui/src/slash_command.rs) and [CLI reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli#built-in-slash-commands) | [72 names](../crates/kiln/src/app/agent_gui/slash_catalog/codex.tsv) |
| Claude Code | 2.1.329 initialization catalog and [official command reference](https://code.claude.com/docs/en/commands#all-commands) | [137 names](../crates/kiln/src/app/agent_gui/slash_catalog/claude.tsv) |
| Antigravity | 1.5.13 native help and ACP advertisements with the [CLI command reference](https://antigravity.google/docs/cli/reference#core-slash-commands) | [61 names](../crates/kiln/src/app/agent_gui/slash_catalog/antigravity.tsv) |

The lists are a reference for completion. CLI versions, feature flags, platform,
account policy and installed extensions determine which names can run. Claude's
retired commands and internal workflow transport endpoints are outside its
public catalog. Codex debug-only entries are labeled in the catalog.

Kiln routes supported operations to app views and native session controls.
Claude Code and Antigravity can also advertise commands for their structured
conversation input. Commands that require provider dialogs prepare the same
pane's CLI frontend through the existing ownership handoff. The CLI input button
inserts the exact command after the new frontend starts. Pressing Enter in the
native prompt runs it and leaves provider confirmations in place.

The CLI input route matters for correctness. An isolated Codex 0.171.0 test
showed that supplying `/pwd` as a resume prompt produced a normal model message.
Pasting it into the live composer and pressing Enter ran the local slash handler.
In an isolated Antigravity 1.5.13 test, `--prompt-interactive /help` started a model
turn, while entering `/help` in the CLI opened its help panel. Those probes used
local or offline endpoints. See [Codex startup prompt handling](https://github.com/openai/codex/blob/rust-v0.171.0/codex-rs/tui/src/chatwidget/input_restore.rs#L215-L255)
and [Antigravity CLI flags](https://antigravity.google/docs/cli/reference#command-line-flags).

For end-user steps and MCP visibility, see [Agents](../site/_build/docs-src/en/agents.md).
