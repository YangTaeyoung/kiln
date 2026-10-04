# Kiln macOS IME trigger patch

Base: crates.io `winit` **0.30.13**, upstream revision
`e9809ef54b18499bb4f2cac945719ecc2a61061b` (the registry archive's VCS record).
The Apache-2.0 license is retained and included in bundled acknowledgements.
The crate is vendored so builds and future releases use the reviewed source,
without modifying anyone's global Cargo registry or tracking a moving fork.

## Narrow source delta

Only `src/platform_impl/macos/view.rs` changes runtime behavior:

- Clear native marked text immediately after committing composition.
- Forward a subsequent `insertText` callback from the same native key event as
  normal keyboard input. Allow `doCommandBySelector` after commit, preserving
  trigger punctuation, digits and control keys.
- Track committed text within one `keyDown`. If the IME already included a
  printable trigger (notably Space) in that commit, suppress its duplicate key
  event. Reset the record before every next key event. Ctrl, Command and Option
  combinations are excluded so shortcuts remain intact.

The first two changes adapt [upstream PR #4478](https://github.com/rust-windowing/winit/pull/4478),
by Jay Choi (`dfb23bff7d1c945a580673a9977f2699e0234d91`). It is a proposal,
not a released upstream fix. The third change covers the duplicate-space
callback sequence caught by Kiln's regression after the initial adaptation.
There is no guessed punctuation injection, delayed key replay, or key-up synthesis.
Non-macOS backends are unchanged. Normalized Cargo.toml is unchanged.

## Verification and removal

```sh
cargo test --locked -p kiln --test native_ime
cargo test --locked -p egui-winit --lib kiln_ime_tests
cargo test --locked -p kiln --test gui ime_commits_reach_only_the_focused_pty_once_and_cancellation_sends_nothing
```

`native_ime` runs a real hidden WinitView on AppKit's main thread in an isolated
process. Its test-only `interpretKeyEvents:` driver supplies deterministic IME
callbacks, exercising production `keyDown:`, `setMarkedText:`, `insertText:` and
`doCommandBySelector:`. Explicit expectations verify question mark, next question
mark, digit, semicolon, spaces, Enter, Backspace, candidate-confirmation consumption,
normal Latin typing and Ctrl/Command/Option shortcuts. It changes no system input
source and leaves the user's windows and sessions alone.

The regression fails on question-mark delivery with unpatched 0.30.13. The
patched callback test and real isolated PTY delivery test must pass. These tests
do not prove physical IME latency or candidate-popup interaction. Recheck those
on the installed application. Remove this patch when a compatible upstream
release passes the same regressions; do not upgrade winit alone across eframe's
incompatible major API boundary.
