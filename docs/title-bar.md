# Game title bar

`stacked config set title_bar VALUE` takes **`default`**, **`compact`** or
**`hidden`**. The choice applies to the next game window you open.

This setting applies only to Wayland game windows. The X11 runtime uses an
unrelated native window and is unaffected.

Hidden removes the title bar without requesting fullscreen, so a tiled window
keeps its tile. Entering and leaving fullscreen does not restore a hidden bar.
Default remains the default for new installations.

Window controls are absent in Hidden mode. Use your desktop's window shortcuts
to move or close the game, and `stacked config` to change this preference.
For direct `cordial-run` launches, `CORDIAL_TITLE_BAR=hidden` selects the same mode.

With no bar showing, whether Hidden or fullscreen, the bottom row of the window
is a black line and the game draws one row shorter. Without it some compositors
treat the window as hidden behind the game and the text-box editor never
appears. Why: [ADR-056](adr/ADR-056-the-host-window-keeps-one-pixel-the-engine-does-not-cover.md).

The bar's colours follow a separate setting, `theme`: `stacked` (the default)
or `system` for your desktop's own. See [usage.md](usage.md#themes).
