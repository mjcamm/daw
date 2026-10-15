# Inspector

*Status: built 2026-10-16 (`crates/daw-ui/src/inspector.rs`). Agreed with the user as a foundation: the one place to configure a selected thing in detail, reused by later pages.*

## Layout

```text
[ header                                                   ]
[ page                              ][ inspector ][ tray ]
```

- **Tray:** a 32 px strip down the right of every page, under the header, with one button (Lucide `panel-right`). It opens and closes the inspector; this is the only way to.
- **"Something here":** when the selection has settings, the tray icon turns accent blue and, while folded, wears a small accent dot. With nothing to show it is dim. The tooltip says which.
- **Panel:** left of the tray, 280 px to start, resized by dragging its left edge (220 to 520 px). It stays open across pages until closed; open state and width are UI state, not saved.
- **Opening:** selecting something only lights the tray. Arriving through a go-to link opens the panel on the thing it went to (the user's call).
- **Look:** sidebar background, a header with the thing's colour dot and name, then section labels over grouped rounded panels with hairline rows of at least 34 px (`look-and-feel.md`).

## What it shows

| Page | Selection | Shows |
|---|---|---|
| Signals | A notes signal | Pitch range (switch), Envelope (graph and sliders), Velocity clamp (switch) (`motion-signals.md`) |
| Signals | A drawn signal | Nothing yet |
| Arrange, Parameters | | Nothing yet (later: the node's channel strip; maybe block settings) |

## Optional sections

A section that can be left out has an on/off switch at the right of its heading (green when on). Off folds its rows away, leaves a muted line saying what happens instead, and keeps its values for when it is switched back on. Each flip is one edit.

## Editing

Values change by dragging (the app's slider, or a value dragged sideways for pitches), previewed live everywhere the change shows, and sent as one edit on release, so each drag is one undo step.
