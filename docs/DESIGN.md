# Snapoleon design language

Quiet, precise, fast. The UI should feel like an instrument: no chrome that
doesn't earn its place, crisp 1px geometry, numbers in monospace, one confident
accent. The name is a wink (Napoleon → bicorne), so the brand mark is a bicorne
silhouette whose brim is a capture bracket. The wink stays in the logo and the
empty states; the working UI stays serious.

## Color tokens (`snap_ui::Palette`)

| token | dark | light | use |
|---|---|---|---|
| `bg` | `#0E0F13` | `#F7F7F5` | window background |
| `surface` | `#16181E` | `#FFFFFF` | panels, cards |
| `surface_2` | `#1D2028` | `#F0F0EC` | inputs, hovered rows, tool rail |
| `elevated` | `#242833` | `#FFFFFF` | menus, popovers, floating toolbars |
| `border` | `#2C303B` | `#E3E2DC` | 1px dividers, control outlines |
| `text` | `#ECEDEF` | `#17181C` | primary text |
| `text_dim` | `#9AA0AC` | `#62646B` | secondary text, captions |
| `text_faint` | `#5D6270` | `#A3A49E` | placeholders, disabled |
| `accent` | `#6B7FFF` | `#3D5AFE` | "Imperial blue": selection, primary buttons, focus |
| `accent_soft` | accent @ 16% | accent @ 12% | selected tool / nav item background |
| `on_accent` | `#FFFFFF` | `#FFFFFF` | text on accent |
| `gold` | `#E3B341` | `#B8860B` | "Laurel gold": recording, step badges, favorites. Sparingly. |
| `danger` | `#FF5A5F` | `#E5484D` | destructive, record dot |
| `success` | `#30C57A` | `#2A9D62` | done states |
| `warning` | `#F5A524` | `#C77A00` | warnings |
| `scrim` | `#08090C` @ 55% | same | overlay dim outside selection |

The user can override `accent` in settings; derive `accent_soft` from it.

## Type

Inter (UI) and JetBrains Mono (numbers, sizes, hex values, hotkeys, code).

| style | font | size / weight |
|---|---|---|
| caption | Inter | 11 / regular, `text_dim` |
| body | Inter | 13 / regular |
| body_strong | Inter | 13 / semibold |
| title | Inter | 16 / semibold |
| headline | Inter | 22 / bold, tracking tight |
| mono | JetBrains Mono | 12 / medium |

Sentence case everywhere. No exclamation marks. Labels are verbs for actions
("Copy", "Upload"), nouns for places ("Library").

## Space, shape, depth

- 4-pt grid: 4, 8, 12, 16, 24, 32. Default item spacing 8; section gaps 24.
- Radius: 6 controls, 10 cards/inputs, 14 windows & panels, fully round pills
  for floating toolbars and chips.
- Borders are 1px `border`; focus ring is 2px `accent` outside the control.
- Shadows only on floating things (toolbars, menus, pins, toasts):
  `0 8 24 #000 @ 35%` dark / `@ 14%` light.
- Controls are 28px tall (compact) / 32px (default). Hit targets ≥ 28px.

## Motion

egui `animate_bool`/`animate_value`: 100 ms hover fades, 160 ms panel
slides, 200 ms toast enter. Selection handles and the loupe never animate
(they must track the cursor exactly). Respect "reduce motion" by setting the
animation time to 0.

## Iconography

Phosphor (regular weight) via `egui-phosphor`. 16px in bars, 18px in the
editor tool rail, 28px in empty states. Icon-only buttons always have a
tooltip with the hotkey in a `kbd` chip.

## Surfaces

**Capture overlay.** Frozen screenshot at 100%; outside the selection is
`scrim`. Selection: 1px `accent` border + 1px white inner hairline, 8px square
handles (white fill, accent border) at corners/edges. Size label: mono pill
`1280 × 720` on `elevated`, top-left above the selection (inside if no room).
Crosshair: 1px dashed lines at 40% white across the monitor before dragging.
Loupe: 120px circle, 8× zoom, pixel grid, center pixel outlined, under it a
mono readout `#3D5AFE  rgb(61, 90, 254)` and cursor coordinates. Window/element
snap: hovered target gets an accent 2px outline and 8% accent fill. Instant
toolbar: floating pill under the selection (flips above at screen bottom):
`Copy · Save · Edit · Pin · Upload | Text · QR · Redact | Record · Scroll | ✕`.

**Editor.** Custom title bar. Left vertical tool rail (44px, `surface_2`), top
context bar shows options for the active tool only (color swatches, stroke,
fill, font size, style variants). Canvas centered on `bg` with a subtle
checkerboard for transparent areas and a 1px `border` around the image. Right
panel (collapsible, 280px): Beautify / Text (OCR) / Export. Bottom status bar:
zoom %, image size, export size. Primary action (Copy) is the accent button at
the top right, with Save, Upload, Pin next to it.

**Library.** Title bar with search field (⌘/Ctrl+F focus) spanning the top,
filter chips below (All, Screenshots, Recordings, Favorites, apps…). Responsive
grid of 16:10 cards (radius 10) with the thumbnail, app icon glyph + title,
relative time; hover reveals quick actions. Selecting opens a right detail
drawer with a large preview, context (app, window, URL), OCR text, links with
copy-as menu, tags.

**Settings.** 200px left sidebar with icon + label nav items; content is a
scroll area of `section`s containing `setting_row`s (label + description left,
control right). Changes apply immediately (no Save button); destructive ones
confirm inline.

**Pins.** Borderless image with 1px border in `accent` @ 60% and a soft
shadow; a tiny hover bar (opacity, zoom %, close) fades in at the top.

**Recorder HUD.** Four 2px `danger` frame windows around the region (dashed
when paused) and a compact pill: red dot + mono timer, pause, mic meter, stop,
discard. Excluded from capture.

**Toasts.** Bottom-right stack in-app (and OS notifications when no window is
open): icon, one-line message, optional action button ("Open", "Copy link").

## Empty states

Centered 28px icon in `text_faint`, title in `title`, one sentence in
`text_dim`, one action. The Library empty state shows the bicorne mark and
"Nothing here yet. Press PrintScreen to take your first shot."
