# Language and native menus

Choose **Settings → Appearance → Language** to use Korean, English, Japanese,
or Simplified Chinese. Changes apply immediately and are saved for the next launch.
Terminal output, filenames, code, commit messages and agent requests retain their
original contents. Existing installations keep Korean until a language is selected.

The macOS menu-bar icon opens the same menu with either mouse button. **Settings…**
opens the existing Kiln window, or launches it if closed. **End Background Sessions…**
shows the live sessions and asks for confirmation. It includes sessions in open
workspaces; unsaved shell/agent work can be lost. Cancel is the default. Sessions
created after confirmation are not included, and the daemon remains available.
The active application's **Kiln → Settings…** menu also uses `⌘,`.

## Maintaining translations

Application messages live in
[`messages.json`](../crates/kiln-common/locales/messages.json). Korean source text
is the key; each entry supplies `en`, `ja` and `zh-CN`. Use distinct source text for
different meanings, such as “Custom” versus “User”, and write complete messages
instead of assembling grammatical fragments.

- Use `kiln_common::i18n::tr("source")` for application-owned labels.
- Use `kiln_common::trf!("source {name}", …)` for formatted messages. All translations
  compile as ordinary Rust `format!` expressions; preserve argument names, order,
  format specs and escapes. Never translate the formatted result or user data.
- Translate constant metadata when rendering, so an open view follows a language
  change. Never translate protocol keys, file formats, CLI commands, or the stored
  agent-request delimiter and its byte offset.
- The preference is stored separately in `language.json` under Kiln's config
  directory. The menu-bar companion synchronizes it while running. A locale
  change reinstalls regional CJK fonts and invalidates terminal glyph caches.
- Tests use scoped language overrides or isolated config/socket paths. Never use
  the real session daemon to test the destructive menu action.

The checker also rejects untranslated literals at direct UI text sinks and requires
all literal translation keys, including English Git labels, to exist in the catalog.
Only test-only items are excluded; production UI after an inline test module
remains checked. Lexer regression tests cover nested comments, raw strings,
character literals, conditional test guards, and unchanged source line numbers.
Product names, key chords, commands, and connection examples use an explicit
allowlist. This guard complements the rendered-language tests; it does not infer
meaning from arbitrary runtime text. Activity notifications store their event type
so their titles follow the selected language after restarting. Agent-supplied
notification titles and bodies retain their original content. Generated workspace
instructions follow the selected language, while existing saved requests keep
their original delimiter and contents.

Run:

```sh
python3 scripts/test-check-localizations.py
python3 scripts/check-localizations.py
cargo test -p kiln-common -p kiln-i18n-macros
cargo test -p kiln --lib
cargo test -p kiln --test gui
```

Inspect the four-language screenshots as well as test output. GUI integration
checks translated settings, persistence, unchanged PTYs/user text, and font-atlas
coordinates after language switching. The native AppKit click path still requires
a macOS UI smoke test; headless tests do not prove menu presentation.

Font provenance and licensing: [bundled CJK fonts](../crates/kiln-common/fonts/README.md).
Formatting reference: [Rust formatting](https://doc.rust-lang.org/std/fmt/).
