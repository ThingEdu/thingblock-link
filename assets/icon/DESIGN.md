# ThingBlock — Brand & Design Reference

Design assets and tokens for the **thingblock-link** helper's native UI (tray icon, status
window, about dialog). These are exported from the `thingblock-editor` editor so the helper's
native surface stays visually consistent with the browser editor it backs.

> **Source of truth.** The canonical assets live in `thingblock-editor`:
> - Wordmark: `packages/scratch-gui/src/components/menu-bar/thingblock-logo.jsx`
> - App glyph: `packages/scratch-gui/static/favicon.svg`
>
> The files here are flattened copies for native consumption. If the brand changes upstream,
> re-export rather than editing these by hand.

## Assets in this folder

| File | What | Use for |
| - | - | - |
| `thingblock-icon.svg` | Square "chip" glyph (32×32 grid) | App/tray icon, window icon, installer |
| `thingblock-icon.ico` | Multi-size Windows icon (16/32/48/256) | Windows tray + `.exe` icon |
| `icons/icon-<size>.png` | Pre-rendered glyph: 16, 22, 24, 32, 48, 64, 128, 256 | Tray (per-platform), GTK/Linux |

PNGs are 8-bit straight-alpha RGBA, which is what the `tray-icon` / `image` crates expect.

## Color palette

The brand is a two-tone gradient pair — a warm **orange** and a fresh **lime** — split across a
microcontroller-chip silhouette, all on a soft dark-green drop shadow.

| Token | Hex | Notes |
| - | - | - |
| Orange (light stop) | `#FFD166` | top of orange gradient |
| Orange (dark stop) | `#FF7B35` | bottom of orange gradient |
| Lime (light stop) | `#D9F99D` | top of lime gradient |
| Lime (dark stop) | `#84CC16` | bottom of lime gradient |
| Shadow | `#0D2800` | drop shadow, 75% opacity |
| Outline / pins | `#FFFFFF` | chip border + connector pins |

Both gradients run **top → bottom** (vertical). Solid-fill fallbacks: use `#FF7B35` for orange
and `#84CC16` for lime.

```rust
// Rust constants if you need them in code (egui/iced/tao status surfaces, etc.)
pub const TB_ORANGE_LIGHT: [u8; 3] = [0xFF, 0xD1, 0x66];
pub const TB_ORANGE_DARK:  [u8; 3] = [0xFF, 0x7B, 0x35];
pub const TB_LIME_LIGHT:   [u8; 3] = [0xD9, 0xF9, 0x9D];
pub const TB_LIME_DARK:    [u8; 3] = [0x84, 0xCC, 0x16];
pub const TB_SHADOW:       [u8; 3] = [0x0D, 0x28, 0x00];
```

## Typography

The wordmark is set in **Fredoka One** (rounded, friendly display face) with a white outline drawn
behind the fill (`paint-order: stroke fill`). Don't re-typeset the wordmark in native code — ship
`thingblock-logo.svg` and render it. For native body/label text, use the platform default UI font;
the brand font is reserved for the wordmark.

## Icon vs. wordmark

- **Glyph** (`thingblock-icon.svg`) is the only mark that survives at small sizes — use it
  everywhere space is square or tight: tray, taskbar, window chrome, installer.
- **Wordmark** (`thingblock-logo.svg`) needs horizontal room — use it in the about dialog,
  splash, or a status window header, never in the tray.

The glyph reads as a chip: white connector pins top and bottom, a rounded body with a diagonal
orange→lime split, a white border, and a single pin-1 dot in the top-left corner.

## Tray icon guidance (`tray-icon` crate)

The crate wants raw RGBA pixels, not a file path. Decode one of the PNGs and hand the bytes over:

```rust
use tray_icon::Icon;

fn load_tray_icon() -> Icon {
    // Embed at compile time so the binary is self-contained.
    let png = include_bytes!("../assets/icon/icons/icon-32.png");
    let img = image::load_from_memory(png)
        .expect("valid tray icon png")
        .into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).expect("rgba -> tray icon")
}
```

Sizing notes:
- **Windows**: 16 or 32 px; the `.ico` carries both. **macOS**: a ~22 px template-ish glyph reads
  best (this glyph is full-color, not a monochrome template — fine for a colored menu-bar mark).
  **Linux** (GTK/AppIndicator): 22–24 px is typical, but ship 32/48 too since DEs upscale.
- When in doubt, embed `icon-32.png`; it's the safe cross-platform default.

### State cues

`tray.rs` already tracks a `Status` (`Starting` / `Running(port)` / `Failed`). If you want the icon
itself to reflect state, prefer a small overlay/tint over redrawing the glyph:
- **Running** → full-color glyph (as shipped).
- **Starting** → desaturated or 60% opacity.
- **Failed** → red dot overlay (reuse the pin-1 dot position, top-left), or tint the lime half toward
  the error red `#FF661A` used by the editor.

Keep the status text in the menu line (as it is now) as the authoritative signal; the icon tint is
a glanceable supplement, not a replacement.

## UI surface guidance

If the helper grows a real window (status panel, about dialog) beyond the tray menu:
- Lead with the **wordmark** in the header, glyph in the corner/window icon.
- Background: near-white (`#FFFFFF` / very light neutral) — the brand is bright and reads best on
  light surfaces. The shadow color `#0D2800` is for the mark's own drop shadow, not a UI background.
- Accent actions/links in **orange** (`#FF7B35`); use **lime** (`#84CC16`) for success/"connected"
  states and the orange-to-red shift for errors.
- Match the editor's voice: rounded, friendly, low-ceremony. Short status strings ("Running on
  :8765", "Connecting…", "Couldn't reach arduino-cli").
