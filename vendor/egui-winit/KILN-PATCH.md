# Kiln IME cancellation patch

This is the crates.io `egui-winit` 0.36.2 source, pinned by Kiln's Cargo.lock.
Upstream source revision: `49682f8baa058bf49e011035cfbd6e825f88a5ef`.
Original files came from the local Cargo registry; license texts came from that
exact upstream revision. Both upstream licenses are retained.

## Delta

- `src/lib.rs`: translate native `winit::event::Ime::Disabled` into an empty
  `egui::ImeEvent::Preedit`, instead of dropping it. Do not synthesize Commit.
- A backend regression test exercises Preedit → Disabled → Enabled and a later
  Commit through the actual `State::on_ime` handler.
- Normalized Cargo.toml license include paths point to the vendored license files.

On macOS, winit 0.30.13 emits Disabled when input sources change or composition is
interrupted. The previous bridge discarded that event, leaving the terminal's
local preedit overlay visible. Empty Preedit is egui 0.36's cancellation signal;
forwarding deprecated egui Enabled/Disabled variants would not repair consumers.

References:
- [winit Disabled contract](https://docs.rs/winit/latest/winit/event/enum.Ime.html#variant.Disabled)
- [egui Preedit contract](https://docs.rs/egui/latest/egui/enum.ImeEvent.html#variant.Preedit)
- [upstream bridge source](https://github.com/emilk/egui/blob/49682f8baa058bf49e011035cfbd6e825f88a5ef/crates/egui-winit/src/lib.rs)

Run `cargo test --locked -p egui-winit --lib kiln_ime_tests` and Kiln's terminal
IME + GUI tests when updating this patch. Remove the patch and vendored source
once the resolved upstream integration preserves cancellation; retain the
regression coverage. `scripts/collect-licenses.py` includes vendored licenses in
the shipped acknowledgements.
