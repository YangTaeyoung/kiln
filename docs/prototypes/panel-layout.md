# Pane layout interaction prototype

[Open the self-contained HTML prototype](panel-layout.html).

This browser prototype is for interaction review before the native pane layout changes are deployed. It uses synthetic content and never connects to real terminal sessions. The existing Kiln visual system remains authoritative; browser colors and illustrative agent marks are approximations, not a new native design system.

## Review the interaction

- Choose **정렬된 4분할**. Hover the central junction to preview the complete cross. Drag horizontally or vertically to choose which complete boundary moves.
- Grab a horizontal or vertical boundary away from its junction to resize only that local segment. The continuous highlight previews the extent before pointer-down and while dragging.
- In **엇갈린 4분할**, align the independent horizontal boundaries before checking the junction behavior. A continuous boundary can be divided into local segments where the two perpendicular cuts coincide.
- Snap guides use the entire terminal area: 1/2, 1/3, 2/3, 1/4, and 3/4. Parent-pane fraction guides are excluded. Adjacent fixed divider alignment remains available.
- Drag a pane header to another pane's edge. The drop preview uses the resulting layout after removing the source pane; pane identities are retained.
- Hold **Alt / Option** to bypass snap and linked movement, press **Escape** to cancel, or use **되돌리기** to restore the preceding layout. Divider arrow keys also resize; the junction retains keyboard focus for repeated linked adjustments.

## Current implementation boundaries

The linked-axis proximity is 64 CSS pixels along a boundary. Hovering toward an aligned junction lights the opposite segment before reaching the central hit region; moving away restores the local-segment preview. Hover selection and pointer-down share this threshold, while an active drag retains its initially chosen scope. The junction has an invisible cross-shaped hit region extending 20 pixels along each boundary. There are no permanent cross marks, grips, or range lines. Boundary scope appears only on hover, keyboard focus, or drag; the resting workspace emphasizes terminal content. Whole-boundary highlights are a single continuous strip; the affected layout region has one outline. Hovering the junction previews one cross, then the first drag direction selects the actual axis. Localizing a continuous boundary transposes equivalent aligned split trees without changing pane rectangles or identities before resizing. An unaligned split remains a continuous boundary until its perpendicular cuts are aligned.

This is a review artifact, not a published app build. The user accepted the interaction on October 6, 2026 and authorized its native implementation, with the final captured-line brightness and thickness chosen during implementation. Do not infer public release completion from prototype checks.

## Validation

The development browser verified local vertical-segment movement, complete vertical-axis movement at the junction, adjacent alignment, linked movement, and whole-area fraction guides. At a 620-pixel viewport the page had no horizontal overflow. Independent DX regression review covered both orientations, hover selection, repeated junction keyboard input, undo, drop-preview geometry, and minimum pane sizes, including uneven nested splits.

## References

- [CSS clipping shapes](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/clip-path)
- [Pointer events and cancellation](https://developer.mozilla.org/en-US/docs/Web/API/PointerEvent)
- [cmux pane-movement CLI contract](https://github.com/manaflow-ai/cmux/blob/main/docs/cli-contract.md)

The snapping and junction rules above are Kiln's user-reviewed specification; these references do not imply cmux implements the same rules.

## Review disposition

Independent design and DX reviews closed their findings. The final fresh visual review returned `ship` for this HTML prototype after correcting linked-off and Option previews. This disposition applies only to the review artifact. The user has authorized native implementation; native verification and distribution are recorded separately.


## Visual feedback rule

Pane movement and resizing reveal destination areas, boundary lines, and affected-region emphasis only during hover, focus, or manipulation without visible instruction badges, direction labels, fraction bubbles, hover tooltips, or status narration. Accessible names and live announcements remain available without adding visible text. Insufficient drop space has a distinct blocked preview. The native implementation must follow the same rule in [DESIGN.md](../../DESIGN.md).


## Magnetic snap feedback

Snap acquisition uses an 18-pixel radius. A captured target stays locked until the pointer is more than 30 pixels away, then releases. The captured boundary and whole-area guide become brighter and thicker while locked, without text badges. Option and disabled snapping clear the lock. Active drags keep their initially selected local or linked scope.
