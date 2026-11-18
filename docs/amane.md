# Pyrrha: amane gotchas

Limits of the amane library the shell works around. `recommendations.md` has
them written up for upstream.

- **Keys:** no modifiers and no key repeat in panels. Delete, Insert, Page
  Up/Down, F-keys and lone modifiers all arrive as `Key::Other`. A focused
  `TextInput` takes every character (so search panels can't use letter keys).
- **D-Bus:** `Argument` can't build structs (blocks the polkit agent, item 10)
  and `Method` can't send error replies. Incoming structs arrive as
  `Value::List`.
- **Images:** PNG and JPEG only (detected by first bytes). Memory is the catch
  (details and numbers under "Image memory" in `recommendations.md`):
  - each request decodes on its own thread, with no limit;
  - a picture is always decoded at **full size** first, a PNG held twice
    over (~8 bytes a pixel), and `.thumbnail(w, h)` only shrinks it after;
  - a thumbnail is kept until the shell exits, **whatever its size**;
    `.blurred(r)` blurs it once;
  - a full-size image is freed when no window draws it anymore, but one that
    was decoded and never drawn is never freed (`Image::loaded()`, which
    decodes at full size, or a view that moved on before the decode
    finished);
  - nothing says whether a thumbnail is ready.

  So never hand amane a big original: wallpapers go through ready-sized
  copies (`src/wallpaper_copies.rs`, item 54). Album art and clipboard
  images still go to amane directly: album art is small, and a clipboard
  screenshot costs one full-size decode while the clipboard panel is open.
- **Widgets:** `ScrollArea` scrolls vertically only and can't be scrolled from
  code (lists window their rows instead). Calling `.height(x)` on a value
  returned as a built `Rectangle` can resolve to the `Widget` getter; pass
  sizes into helpers instead (why `ui::heading` takes a height).
  `Rectangle::fill` takes a `Color` or an `Image`.
- **Canvas hangs this GPU:** `Canvas` shapes (`Arc`, `Line`, `Circle`,
  `Path`) are drawn through vello's compute shaders, and stroked arcs hung the
  APU (gfx_v8; kernel "amdgpu: ring gfx timeout … Process amane-shell", then a
  GPU reset that kills the shell, 2026-11-16). Signs before the crash: grim
  takes 15+ s and keys seem ignored, as frames stop. Don't use `Canvas`; build
  shapes from `Rectangle`s (`.radius(Full)` for dots, borders for outlines).
  Rectangles only stay off vello while their transform moves and scales
  evenly (`graphics/gpu/paint.rs`, `route`): `.rotate()`, gradients, blur,
  shaders and rotated clips all go to vello too, so no diagonals either.
  Check `journalctl -k | grep -i "gfx timeout"` after trying any new kind of
  drawing.
- **Letters share one 1024 px atlas** per renderer; letters that don't fit are
  left out (a three-letter numeral at 600 px lost its last one), so keep text
  under about 0.15 × the monitor height.
- **Text:** letters are drawn from outlines only, so colour emoji draw as
  blank space; Nerd Font glyphs and outline symbols (♥ ★) are fine.
- **Monitors:** `Monitor` has `name`, `width`, `height` in widget units, but no
  scale factor.
- **Layers:** `Layer::Background|Bottom|Top|Overlay`, `Zone::Reserve|Respect|
  Ignore`, `Keyboard::None|Exclusive|OnDemand`; mango gives exclusive focus to
  any exclusive surface, even hidden ones.
- **Idle cost:** amane's `Apps` rescans icon themes every 30 s and `Network`
  re-reads saved profiles each second once started; accepted.
- The upstream list in `recommendations.md` has these written up as issue texts.

