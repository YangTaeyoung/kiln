# Arrange your panes

Drag a pane's title to the left, right, top, or bottom of another pane. The
highlight shows its destination in the resulting layout. Moving a pane preserves
its terminal session and running process; it does not open a replacement shell.

Drag a divider to resize panes. Away from an aligned intersection, only that
segment moves. Near the intersection, connected segments highlight together and
move as one boundary. The selected range stays fixed for that drag. At the
intersection itself, the initial horizontal or vertical movement chooses the axis.

Dividers snap to halves, thirds, and quarters of the entire terminal area, and to
adjacent divider positions. A captured line becomes brighter and thicker and stays
attached through a short release margin. Hold Option to bypass snapping and linked
movement. Press Escape to restore the layout from before the drag.

Layout controls remain quiet when idle. Destination and range highlights appear
only during interaction. A blocked destination cannot change the layout.

For maintainers, the accepted interaction specification is recorded in the
[prototype notes](prototypes/panel-layout.md) and [design rules](../DESIGN.md).
