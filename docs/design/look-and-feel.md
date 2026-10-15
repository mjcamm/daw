# Look and Feel

*Status: direction agreed. Source: the "Daylight" mockups in `docs/ui-ideas/` (arrangement and mixer, light and dark). They set the vibe, not the exact layout.*

## The vibe

A calm, native-feeling "pro app" in the Apple tradition: quiet grey chrome, one blue accent, and colourful content. Clips, layers and signals carry the colour; everything around them stays neutral and recedes.

- Small, dense type (11–13 px), with numbers (time, dB, Hz) in a monospaced font with tabular figures.
- Grouped inset panels with rounded corners and hairline separators, like system settings.
- Content colours are soft tinted fills with a brighter outline and brighter detail (waveforms, notes, curves), never heavy solid blocks.
- Dark and light themes are equal citizens. No colour is ever hard-coded in a view; everything comes from theme tokens.

## Layout pattern

The mockups set the vibe, not the layout. Each page's layout is decided with the user before it is built.

- **Header bar** (about 52 px), left to right: song name and details, page tabs (Arrange · Parameters · Signals · Mixer), then the transport.
- **Arrange page:** node list on the left, one row per node in the style of traditional tracks (colour, name, meter, mute, solo), with the timeline beside it. No sidebar or inspector for now.

## Tokens

Values from the mockups. Light theme / dark theme.

### Surfaces

| Token | Light | Dark | Use |
|---|---|---|---|
| `window` | `#FFFFFF` | `#1E1E1E` | Main content background |
| `sidebar` | `#F5F5F7` | `#262628` | Sidebars, inspector |
| `group` | `#FFFFFF` | `#2C2C2E` | Grouped panels in the inspector |
| `hover` | `rgba(0,0,0,.05)` | `rgba(255,255,255,.08)` | Hovered controls |
| `control` | `rgba(0,0,0,.06)` | `rgba(255,255,255,.1)` | Small buttons (M, S), segmented controls |
| `hairline` | `rgba(0,0,0,.1)` | `rgba(255,255,255,.09)` | Separators, grid lines |
| `edge` | `rgba(0,0,0,.18)` | `rgba(0,0,0,.6)` | Panel borders |

### Text

| Token | Light | Dark |
|---|---|---|
| `text` | `rgba(0,0,0,.85)` | `rgba(255,255,255,.86)` |
| `text-secondary` | `rgba(0,0,0,.5)` | `rgba(255,255,255,.5)` |
| `text-tertiary` | `rgba(0,0,0,.25)` | `rgba(255,255,255,.3)` |

### System colours

| Token | Light | Dark |
|---|---|---|
| `accent` | `#007AFF` | `#0A84FF` |
| `record` | `#FF3B30` | `#FF453A` |
| `active` (toggles, meters low) | `#34C759` | `#30D158` |
| `warning` (mute) | `#FF9500` | `#FF9F0A` |

### Content palette

Nine colours for nodes and signals. Light theme pairs a pale fill with a deep detail colour; dark theme uses one bright colour, tinted for fills (18% alpha) and outlines (35% alpha).

| # | Light fill / deep | Dark bright |
|---|---|---|
| 1 Red | `#FBDCD6` / `#C2402B` | `#FF8A75` |
| 2 Orange | `#FCE6CC` / `#B4600F` | `#FFB061` |
| 3 Yellow | `#F6EDC2` / `#86700A` | `#E8CF5A` |
| 4 Green | `#D9F0D2` / `#2E7D32` | `#7DD87A` |
| 5 Teal | `#CFEDEA` / `#127567` | `#5FD3C2` |
| 6 Blue | `#D3E5FA` / `#1E62B0` | `#6AAFFF` |
| 7 Purple | `#E2DDFA` / `#5946C9` | `#A99BFF` |
| 8 Pink | `#F7D9EF` / `#A23488` | `#F28AD8` |
| 9 Grey | `#E8E8EA` / `#5E5E63` | `#A1A1A6` |

Muted content drops to grey (`#6E6E73` in dark) at low fill opacity.

### Type

- UI font: the platform's system UI font (SF Pro on macOS, Segoe UI on Windows). Sizes 10, 11, 12, 13, 15 px; 12 px is the base.
- Numbers: monospaced with tabular figures (SF Mono, Cascadia Mono/Consolas on Windows).
- Weights: regular for content, semibold (600) for labels such as clip and section names.

### Shape

- Corner radii: 2 px (clip tags), 4–7 px (buttons, clips, segmented controls), 10 px (grouped panels), 12 px (window).
- Separators and outlines are 0.5 px hairlines.
- Rows in grouped panels are at least 34 px tall with 12 px side padding.
