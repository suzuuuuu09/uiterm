//! Native `libghostty-vt` wrapper: VT state, RenderState dirty tracking,
//! key/mouse/paste encoding, selection gestures, and headless throughput benchmark.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use ghostty_vt::Error as VtError;
use ghostty_vt::fmt::Format;
use ghostty_vt::key::{
    Action as KeyAction, Encoder as KeyEncoder, Event as KeyEvent, Key, KittyKeyFlags, Mods,
    OptionAsAlt,
};
use ghostty_vt::mouse::{
    Action as MouseAction, Button as MouseButton, Encoder as MouseEncoder,
    EncoderSize as MouseEncoderSize, Event as MouseEvent, Position as MousePosition,
};
use ghostty_vt::paste;
use ghostty_vt::render::{
    CellIterator, CursorVisualStyle, Dirty, RenderState, RowId, RowIterator, Snapshot,
};
use ghostty_vt::screen::{CellWide, Screen};
use ghostty_vt::selection::FormatOptions;
use ghostty_vt::selection::gesture::{
    DragEvent as SelectionDragEvent, Geometry as SelectionGeometry, Gesture as SelectionGesture,
    PressEvent as SelectionPressEvent, ReleaseEvent as SelectionReleaseEvent,
};
use ghostty_vt::style::{Palette, PaletteIndex, RgbColor, StyleColor, Underline};
use ghostty_vt::terminal::{
    ColorScheme, Event, Mode, Options as TerminalOptions, Point, PointCoordinate, ScrollViewport,
    Scrollbar, Terminal,
};

pub const DEFAULT_SCROLLBACK_LINES: usize = 2000;
pub const DEFAULT_SCROLLBACK_MAX_BYTES: usize = 10 * 1024 * 1024;
pub const DEFAULT_BG_U32: u32 = 0x09090b;
pub const DEFAULT_FG_U32: u32 = 0xf4f4f5;
pub const DEFAULT_CURSOR_U32: u32 = 0xe4e4e7;

const DEFAULT_PALETTE_HEX: [u32; 16] = [
    0x18181b, 0xef4444, 0x22c55e, 0xeab308, 0x3b82f6, 0xc084fc, 0x06b6d4, 0xe4e4e7, 0x52525b,
    0xf87171, 0x4ade80, 0xfacc15, 0x60a5fa, 0xd8b4fe, 0x22d3ee, 0xffffff,
];

fn rgb_from_u32(hex: u32) -> RgbColor {
    RgbColor {
        r: ((hex >> 16) & 0xff) as u8,
        g: ((hex >> 8) & 0xff) as u8,
        b: (hex & 0xff) as u8,
    }
}

pub fn rgb_to_u32(c: RgbColor) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32)
}

#[derive(Debug, Clone)]
pub struct StyledCell {
    pub col: u16,
    pub width: u8,
    pub text: String,
    pub fg_rgb: Option<(u8, u8, u8)>,
    pub bg_rgb: Option<(u8, u8, u8)>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub underline: Underline,
    pub underline_rgb: Option<(u8, u8, u8)>,
    pub strikethrough: bool,
    #[allow(dead_code)]
    pub overline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSnapshot {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
    pub blinking: bool,
    pub visual_style: CursorVisualStyle,
    pub at_wide_tail: bool,
}

#[derive(Debug, Clone)]
pub struct ScreenColorsSnapshot {
    pub fg_u32: u32,
    pub bg_u32: u32,
    pub cursor_u32: Option<u32>,
    pub palette: [RgbColor; 256],
    pub hash: u64,
}

impl ScreenColorsSnapshot {
    pub fn from_snapshot(snap: &Snapshot<'_>) -> Self {
        match snap.colors() {
            Ok(c) => {
                let fg_u32 = rgb_to_u32(c.foreground);
                let bg_u32 = rgb_to_u32(c.background);
                let cursor_u32 = c.cursor.map(rgb_to_u32);
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                fg_u32.hash(&mut hasher);
                bg_u32.hash(&mut hasher);
                cursor_u32.hash(&mut hasher);
                c.palette.hash(&mut hasher);
                Self {
                    fg_u32,
                    bg_u32,
                    cursor_u32,
                    palette: c.palette,
                    hash: hasher.finish(),
                }
            }
            Err(_) => {
                let palette = Palette::default().0;
                Self {
                    fg_u32: DEFAULT_FG_U32,
                    bg_u32: DEFAULT_BG_U32,
                    cursor_u32: Some(DEFAULT_CURSOR_U32),
                    palette,
                    hash: 0,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderFrameMeta {
    pub cols: u16,
    pub rows: u16,
    pub active_screen: Screen,
    pub dirty: Dirty,
    pub cursor: CursorSnapshot,
    pub colors: ScreenColorsSnapshot,
    pub scrollbar: Option<Scrollbar>,
    pub selection_generation: u64,
}

#[derive(Debug, Clone)]
pub struct ScreenSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<Vec<StyledCell>>,
    pub row_selections: Vec<Option<(u16, u16)>>,
    pub cursor: CursorSnapshot,
    pub colors: ScreenColorsSnapshot,
}

#[derive(Debug, Default)]
pub struct VtFeedEvents {
    pub pty_writes: Vec<Vec<u8>>,
    pub title_changed: Option<String>,
    pub bell: bool,
    pub render_hold: Option<bool>,
}

/// Normalizes OSC 133 `A`/`N` prompt markers that omit `redraw=` so Ghostty/libghostty-vt
/// preserves shell prompts across resize (matching `src/terminal/outputProtocol.ts`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum OscState {
    #[default]
    Ground,
    Escape,
    Csi,
    Private,
    Osc,
    String,
}

#[derive(Debug, Clone, Default)]
pub struct Osc133Normalizer {
    state: OscState,
    osc_field: String,
    osc_field_index: u8,
    osc_prompt: bool,
    osc_explicit_redraw: bool,
}

impl Osc133Normalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn normalize<'a>(&mut self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        const PRESERVE_PROMPT: &[u8] = b";redraw=0";
        let mut insertions: Vec<usize> = Vec::new();
        let mut index = 0usize;

        while index < bytes.len() {
            if self.state == OscState::Ground {
                if let Some(rel) = bytes[index..].iter().position(|&b| b == 0x1b) {
                    index += rel + 1;
                    self.state = OscState::Escape;
                    continue;
                } else {
                    break;
                }
            }

            let byte = bytes[index];
            if self.state == OscState::Osc && (byte == 0x07 || byte == 0x1b) {
                self.finish_osc_field();
                if self.osc_prompt && self.osc_field_index == 2 && !self.osc_explicit_redraw {
                    insertions.push(index);
                }
                self.state = if byte == 0x1b {
                    OscState::Escape
                } else {
                    OscState::Ground
                };
                index += 1;
                continue;
            }

            if byte == 0x1b || byte == 0x18 || byte == 0x1a {
                self.state = if byte == 0x1b {
                    OscState::Escape
                } else {
                    OscState::Ground
                };
                index += 1;
                continue;
            }

            if (byte < 0x20 || byte == 0x7f)
                && matches!(
                    self.state,
                    OscState::Escape | OscState::Csi | OscState::Private
                )
            {
                index += 1;
                continue;
            }

            match self.state {
                OscState::Escape => {
                    self.state = OscState::Ground;
                    if byte == b'[' {
                        self.state = OscState::Csi;
                    } else if byte == b']' {
                        self.state = OscState::Osc;
                        self.osc_field.clear();
                        self.osc_field_index = 0;
                        self.osc_prompt = true;
                        self.osc_explicit_redraw = false;
                    } else if matches!(byte, b'P' | b'X' | b'^' | b'_') {
                        self.state = OscState::String;
                    }
                }
                OscState::Csi => {
                    self.state = if byte == b'?' {
                        OscState::Private
                    } else {
                        OscState::Ground
                    };
                }
                OscState::Private => {
                    if !matches!(byte, b'0'..=b'9' | b';') && byte >= 0x20 && byte != 0x7f {
                        self.state = OscState::Ground;
                    }
                }
                OscState::Osc => {
                    if byte == b';' {
                        self.finish_osc_field();
                    } else if self.osc_prompt && self.osc_field.len() < "redraw=".len() {
                        self.osc_field.push(byte as char);
                    }
                }
                OscState::String | OscState::Ground => {}
            }
            index += 1;
        }

        if insertions.is_empty() {
            return Cow::Borrowed(bytes);
        }
        let mut out = Vec::with_capacity(bytes.len() + insertions.len() * PRESERVE_PROMPT.len());
        let mut start = 0usize;
        for end in insertions {
            out.extend_from_slice(&bytes[start..end]);
            out.extend_from_slice(PRESERVE_PROMPT);
            start = end;
        }
        out.extend_from_slice(&bytes[start..]);
        Cow::Owned(out)
    }

    fn finish_osc_field(&mut self) {
        if !self.osc_prompt {
            return;
        }
        if self.osc_field_index == 0 {
            self.osc_prompt = self.osc_field == "133";
        } else if self.osc_field_index == 1 {
            self.osc_prompt = self.osc_field == "A" || self.osc_field == "N";
        } else {
            self.osc_explicit_redraw |= self.osc_field == "redraw=";
        }
        self.osc_field_index = self.osc_field_index.saturating_add(1).min(2);
        self.osc_field.clear();
    }
}

pub struct VtTerminal {
    terminal: Terminal,
    render_state: RenderState,
    row_iter: RowIterator,
    cell_iter: CellIterator,
    key_encoder: KeyEncoder,
    key_event: KeyEvent,
    mouse_encoder: MouseEncoder,
    mouse_event: MouseEvent,
    selection_gesture: SelectionGesture,
    selection_press: SelectionPressEvent,
    selection_drag: SelectionDragEvent,
    selection_release: SelectionReleaseEvent,
    selection_origin: Instant,
    selection_generation: u64,
    osc_normalizer: Osc133Normalizer,
    cols: u16,
    rows: u16,
    pub mac_option_as_alt: bool,
    pub sync_hold_since: Option<Instant>,
}

impl VtTerminal {
    pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Result<Self, String> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: cols.max(1),
            rows: rows.max(1),
            max_scrollback: DEFAULT_SCROLLBACK_MAX_BYTES,
        })
        .map_err(|e| format!("ghostty_vt::Terminal::new: {e}"))?;
        let _ = terminal.set_scrollback_max_lines(Some(scrollback_lines));
        let _ = terminal.set_default_bg_color(Some(rgb_from_u32(DEFAULT_BG_U32)));
        let _ = terminal.set_default_fg_color(Some(rgb_from_u32(DEFAULT_FG_U32)));
        let _ = terminal.set_default_cursor_color(Some(rgb_from_u32(DEFAULT_CURSOR_U32)));
        let mut palette = Palette::default();
        for (i, &hex) in DEFAULT_PALETTE_HEX.iter().enumerate() {
            palette.set(PaletteIndex(i as u8), rgb_from_u32(hex));
        }
        let _ = terminal.set_default_color_palette(Some(palette));
        terminal.set_color_scheme(Some(ColorScheme::Dark));

        let render_state = RenderState::new().map_err(|e| format!("RenderState::new: {e}"))?;
        let row_iter = RowIterator::new().map_err(|e| format!("RowIterator::new: {e}"))?;
        let cell_iter = CellIterator::new().map_err(|e| format!("CellIterator::new: {e}"))?;
        let key_encoder = KeyEncoder::new().map_err(|e| format!("KeyEncoder::new: {e}"))?;
        let key_event = KeyEvent::new().map_err(|e| format!("KeyEvent::new: {e}"))?;
        let mouse_encoder = MouseEncoder::new().map_err(|e| format!("MouseEncoder::new: {e}"))?;
        let mouse_event = MouseEvent::new().map_err(|e| format!("MouseEvent::new: {e}"))?;
        let selection_gesture =
            SelectionGesture::new().map_err(|e| format!("SelectionGesture::new: {e}"))?;
        let selection_press =
            SelectionPressEvent::new().map_err(|e| format!("SelectionPressEvent::new: {e}"))?;
        let selection_drag =
            SelectionDragEvent::new().map_err(|e| format!("SelectionDragEvent::new: {e}"))?;
        let selection_release =
            SelectionReleaseEvent::new().map_err(|e| format!("SelectionReleaseEvent::new: {e}"))?;

        Ok(Self {
            terminal,
            render_state,
            row_iter,
            cell_iter,
            key_encoder,
            key_event,
            mouse_encoder,
            mouse_event,
            selection_gesture,
            selection_press,
            selection_drag,
            selection_release,
            selection_origin: Instant::now(),
            selection_generation: 0,
            osc_normalizer: Osc133Normalizer::new(),
            cols: cols.max(1),
            rows: rows.max(1),
            mac_option_as_alt: true,
            sync_hold_since: None,
        })
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn active_screen(&self) -> Screen {
        self.terminal.active_screen().unwrap_or(Screen::Primary)
    }

    pub fn is_mouse_tracking(&self) -> bool {
        self.terminal.is_mouse_tracking().unwrap_or(false)
    }

    pub fn is_alternate_scroll_enabled(&self) -> bool {
        self.active_screen() == Screen::Alternate
            && self.terminal.mode(Mode::ALT_SCROLL).unwrap_or(false)
    }

    pub fn is_viewport_active(&self) -> bool {
        self.terminal.is_viewport_active().unwrap_or(true)
    }

    #[allow(dead_code)]
    pub fn kitty_keyboard_flags(&self) -> KittyKeyFlags {
        self.terminal
            .kitty_keyboard_flags()
            .unwrap_or(KittyKeyFlags::DISABLED)
    }

    /// Feeds raw PTY bytes into `libghostty-vt` and drains VT events (`PtyWrite`, title, bell, sync hold).
    pub fn feed(&mut self, bytes: &[u8]) -> VtFeedEvents {
        let normalized = self.osc_normalizer.normalize(bytes);
        self.terminal.vt_write(&normalized);
        self.drain_events()
    }

    /// Raw feed without OSC 133 allocation check (for pure VT benchmarks).
    pub fn feed_raw(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.terminal.vt_write(bytes);
        self.drain_events().pty_writes
    }

    fn drain_events(&mut self) -> VtFeedEvents {
        let mut out = VtFeedEvents::default();
        for ev in self.terminal.events() {
            match ev {
                Event::PtyWrite(reply) => out.pty_writes.push(reply),
                Event::TitleChanged(title) => out.title_changed = Some(title),
                Event::Bell => out.bell = true,
                Event::RenderHold(held) => {
                    self.sync_hold_since = if held { Some(Instant::now()) } else { None };
                    out.render_hold = Some(held);
                }
                _ => {}
            }
        }
        out
    }

    /// Releases a synchronized-output hold (Mode 2026) if an application held it past timeout.
    pub fn release_expired_sync_hold(&mut self, max_hold: Duration) -> bool {
        if let Some(since) = self.sync_hold_since
            && since.elapsed() >= max_hold
        {
            self.sync_hold_since = None;
            let _ = self.terminal.set_mode(Mode::SYNC_OUTPUT, false);
            return true;
        }
        false
    }

    pub fn is_sync_held(&self) -> bool {
        self.sync_hold_since.is_some()
    }

    /// Resizes `libghostty-vt` and returns any Mode 2048 in-band size report `PtyWrite`s.
    /// Callers MUST send `FRAME_RESIZE` to `herdr-core` before sending these `FRAME_AUTOMATIC_INPUT` writes.
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_w_px: u32,
        cell_h_px: u32,
    ) -> Result<Vec<Vec<u8>>, String> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        self.terminal
            .resize(cols, rows, cell_w_px.max(1), cell_h_px.max(1))
            .map_err(|e| format!("Terminal::resize: {e}"))?;
        self.cols = cols;
        self.rows = rows;
        let events = self.drain_events();
        Ok(events.pty_writes)
    }

    pub fn scroll_viewport(&mut self, scroll: ScrollViewport) {
        self.terminal.scroll_viewport(scroll);
    }

    pub fn encode_key(
        &mut self,
        key: Key,
        mods: Mods,
        utf8: Option<&str>,
        unshifted_codepoint: Option<char>,
    ) -> Vec<u8> {
        self.key_encoder.set_options_from_terminal(&self.terminal);
        self.key_encoder
            .set_macos_option_as_alt(if self.mac_option_as_alt {
                OptionAsAlt::True
            } else {
                OptionAsAlt::False
            });
        self.key_event
            .set_action(KeyAction::Press)
            .set_key(key)
            .set_mods(mods)
            .set_consumed_mods(Mods::empty())
            .set_composing(false)
            .set_utf8(utf8);
        self.key_event
            .set_unshifted_codepoint(unshifted_codepoint.unwrap_or('\0'));
        let mut out = Vec::with_capacity(16);
        let _ = self.key_encoder.encode_to_vec(&self.key_event, &mut out);
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode_mouse(
        &mut self,
        action: MouseAction,
        button: Option<MouseButton>,
        mods: Mods,
        x_px: f32,
        y_px: f32,
        any_pressed: bool,
        size: MouseEncoderSize,
    ) -> Vec<u8> {
        self.mouse_encoder
            .set_options_from_terminal(&self.terminal);
        self.mouse_encoder.set_size(size);
        self.mouse_encoder.set_any_button_pressed(any_pressed);
        self.mouse_encoder.set_track_last_cell(true);
        self.mouse_event
            .set_action(action)
            .set_button(button)
            .set_mods(mods)
            .set_position(MousePosition { x: x_px, y: y_px });
        let mut out = Vec::with_capacity(24);
        let _ = self
            .mouse_encoder
            .encode_to_vec(&self.mouse_event, &mut out);
        out
    }

    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        if text.is_empty() {
            return Vec::new();
        }
        let bracketed = self.terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
        let mut src = text.as_bytes().to_vec();
        let mut out = vec![0u8; src.len() + 32];
        loop {
            match paste::encode(&mut src, bracketed, &mut out) {
                Ok(n) => {
                    out.truncate(n);
                    return out;
                }
                Err(VtError::OutOfSpace { required }) => {
                    out.resize(required.max(out.len() * 2), 0);
                }
                Err(_) => return Vec::new(),
            }
        }
    }

    pub fn selection_press(&mut self, col: u16, row: u16, x_px: f64, y_px: f64) {
        let clamped_col = col.min(self.cols.saturating_sub(1));
        let clamped_row = row.min(self.rows.saturating_sub(1));
        let Ok(grid_ref) = self.terminal.grid_ref(Point::Viewport(PointCoordinate {
            x: clamped_col,
            y: clamped_row as u32,
        })) else {
            return;
        };
        let elapsed = self.selection_origin.elapsed();
        let _ = self
            .selection_press
            .set_position(x_px, y_px)
            .and_then(|e| e.set_time(elapsed))
            .and_then(|e| e.set_repeat_interval(Duration::from_millis(400)))
            .and_then(|e| e.set_repeat_distance(5.0));
        if let Ok(sel) =
            self.selection_press
                .apply(&mut self.selection_gesture, &self.terminal, grid_ref)
        {
            let _ = self.terminal.set_selection(sel.as_ref());
            self.selection_generation = self.selection_generation.wrapping_add(1);
        }
    }

    pub fn selection_drag(
        &mut self,
        col: u16,
        row: u16,
        px_pos: (f64, f64),
        rectangle: bool,
        geom: (u32, u32, u32),
    ) {
        let (x_px, y_px) = px_pos;
        let (cell_w_px, padding_left_px, screen_h_px) = geom;
        let clamped_col = col.min(self.cols.saturating_sub(1));
        let clamped_row = row.min(self.rows.saturating_sub(1));
        let Ok(grid_ref) = self.terminal.grid_ref(Point::Viewport(PointCoordinate {
            x: clamped_col,
            y: clamped_row as u32,
        })) else {
            return;
        };
        let geometry = SelectionGeometry {
            columns: self.cols as u32,
            cell_width: cell_w_px.max(1),
            padding_left: padding_left_px,
            screen_height: screen_h_px.max(1),
        };
        let _ = self
            .selection_drag
            .set_position(x_px, y_px)
            .and_then(|e| e.set_rectangle(rectangle));
        if let Ok(Some(sel)) = self.selection_drag.apply(
            &mut self.selection_gesture,
            &self.terminal,
            grid_ref,
            geometry,
        ) {
            let _ = self.terminal.set_selection(Some(&sel));
            self.selection_generation = self.selection_generation.wrapping_add(1);
        }
    }

    pub fn selection_release(&mut self, col: u16, row: u16) {
        let clamped_col = col.min(self.cols.saturating_sub(1));
        let clamped_row = row.min(self.rows.saturating_sub(1));
        let grid_ref = self
            .terminal
            .grid_ref(Point::Viewport(PointCoordinate {
                x: clamped_col,
                y: clamped_row as u32,
            }))
            .ok();
        let _ = self
            .selection_release
            .apply(&mut self.selection_gesture, &self.terminal, grid_ref);
    }

    pub fn select_all(&mut self) {
        if let Ok(Some(sel)) = self.terminal.select_all() {
            let _ = self.terminal.set_selection(Some(&sel));
            self.selection_generation = self.selection_generation.wrapping_add(1);
        }
    }

    pub fn clear_selection(&mut self) {
        if self.has_selection() {
            let _ = self.terminal.set_selection(None);
            self.selection_gesture.reset(&self.terminal);
            self.selection_generation = self.selection_generation.wrapping_add(1);
        }
    }

    pub fn has_selection(&self) -> bool {
        matches!(self.terminal.selection(), Ok(Some(_)))
    }

    pub fn selected_text(&self) -> Option<String> {
        let opts = FormatOptions::new()
            .with_emit_format(Format::Plain)
            .with_unwrap(true)
            .with_trim(true);
        let bytes = self.terminal.format_selection_alloc(opts).ok().flatten()?;
        let s = String::from_utf8_lossy(&bytes).into_owned();
        if s.is_empty() { None } else { Some(s) }
    }

    /// Updates `RenderState`, passes snapshot + iterators to the caller, and marks the snapshot clean.
    pub fn render_update<F>(&mut self, f: F) -> Result<(), String>
    where
        F: FnOnce(
            &RenderFrameMeta,
            &Snapshot<'_>,
            &mut RowIterator,
            &mut CellIterator,
        ) -> Result<(), String>,
    {
        let active_screen = self.active_screen();
        let scrollbar = if self.is_viewport_active() {
            None
        } else {
            self.terminal.scrollbar().ok()
        };
        let snap = self
            .render_state
            .update(&self.terminal)
            .map_err(|e| format!("RenderState::update: {e}"))?;
        let cols = snap.cols().unwrap_or(self.cols);
        let rows = snap.rows().unwrap_or(self.rows);
        let dirty = snap.dirty().unwrap_or(Dirty::Full);
        let cursor = read_cursor(&snap);
        let colors = ScreenColorsSnapshot::from_snapshot(&snap);
        let meta = RenderFrameMeta {
            cols,
            rows,
            active_screen,
            dirty,
            cursor,
            colors,
            scrollbar,
            selection_generation: self.selection_generation,
        };
        f(&meta, &snap, &mut self.row_iter, &mut self.cell_iter)?;
        let _ = snap.clean();
        Ok(())
    }

    /// Extracts the full visible viewport grid without cross-frame caching (used by `--bench-window` & tests).
    pub fn snapshot(&mut self) -> Result<ScreenSnapshot, String> {
        let mut out: Option<ScreenSnapshot> = None;
        self.render_update(|meta, snap, row_iter, cell_iter| {
            let mut lines = Vec::with_capacity(meta.rows as usize);
            let mut row_selections = Vec::with_capacity(meta.rows as usize);
            let mut grapheme_buf = String::with_capacity(16);
            let mut row_it = row_iter
                .update(snap)
                .map_err(|e| format!("RowIterator::update: {e}"))?;
            while let Some(row) = row_it.next() {
                let sel = row
                    .selection()
                    .ok()
                    .flatten()
                    .map(|s| (s.start_x, s.end_x));
                row_selections.push(sel);
                lines.push(extract_row_cells(row, cell_iter, &meta.colors, &mut grapheme_buf)?);
            }
            out = Some(ScreenSnapshot {
                cols: meta.cols,
                rows: meta.rows,
                lines,
                row_selections,
                cursor: meta.cursor,
                colors: meta.colors.clone(),
            });
            Ok(())
        })?;
        Ok(out.expect("snapshot populated"))
    }
}

fn read_cursor(snap: &Snapshot<'_>) -> CursorSnapshot {
    match snap.cursor().ok() {
        Some(c) => CursorSnapshot {
            col: c.viewport.as_ref().map(|v| v.x).unwrap_or(0),
            row: c.viewport.as_ref().map(|v| v.y).unwrap_or(0),
            visible: c.visible && c.viewport.is_some(),
            blinking: c.blinking,
            visual_style: c.visual_style,
            at_wide_tail: c.viewport.as_ref().is_some_and(|v| v.at_wide_tail),
        },
        None => CursorSnapshot {
            col: 0,
            row: 0,
            visible: false,
            blinking: false,
            visual_style: CursorVisualStyle::Block,
            at_wide_tail: false,
        },
    }
}

pub fn extract_row_cells(
    row: &ghostty_vt::render::RowIteration<'_>,
    cell_iter: &mut CellIterator,
    colors: &ScreenColorsSnapshot,
    grapheme_buf: &mut String,
) -> Result<Vec<StyledCell>, String> {
    let mut row_cells = Vec::new();
    let mut cell_it = cell_iter
        .update(row)
        .map_err(|e| format!("CellIterator::update: {e}"))?;
    let mut col: u16 = 0;
    while let Some(cell) = cell_it.next() {
        let wide = cell
            .raw_cell()
            .and_then(|c| c.wide())
            .unwrap_or(CellWide::Narrow);
        let is_spacer = matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead);
        let width: u8 = match wide {
            CellWide::Wide => 2,
            CellWide::Narrow => 1,
            CellWide::SpacerTail | CellWide::SpacerHead => 0,
        };
        if !is_spacer {
            grapheme_buf.clear();
            let _ = cell.graphemes_utf8(grapheme_buf);
            let has_styling = cell.has_styling().unwrap_or(true);
            let style = if has_styling {
                cell.style().unwrap_or_default()
            } else {
                ghostty_vt::style::Style::default()
            };
            let fg_rgb = cell.fg_color().ok().flatten().map(|c| (c.r, c.g, c.b));
            let bg_rgb = cell.bg_color().ok().flatten().map(|c| (c.r, c.g, c.b));
            let underline_rgb = match style.underline_color {
                StyleColor::None => None,
                StyleColor::Palette(idx) => {
                    let c = colors.palette[idx.0 as usize];
                    Some((c.r, c.g, c.b))
                }
                StyleColor::Rgb(c) => Some((c.r, c.g, c.b)),
            };
            let has_decor = !matches!(style.underline, Underline::None) || style.strikethrough || style.overline;
            if !grapheme_buf.is_empty() || bg_rgb.is_some() || style.inverse || has_decor {
                row_cells.push(StyledCell {
                    col,
                    width,
                    text: grapheme_buf.clone(),
                    fg_rgb,
                    bg_rgb,
                    bold: style.bold,
                    italic: style.italic,
                    faint: style.faint,
                    inverse: style.inverse,
                    invisible: style.invisible,
                    underline: style.underline,
                    underline_rgb,
                    strikethrough: style.strikethrough,
                    overline: style.overline,
                });
            }
        }
        col = col.saturating_add(1);
    }
    Ok(row_cells)
}

#[allow(dead_code)]
pub fn row_id_value(id: RowId) -> [u64; 2] {
    id.0
}

/// Returns current process resident set size (RSS) in MiB on macOS.
pub fn current_rss_mib() -> f64 {
    #[cfg(target_os = "macos")]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        if rc == 0 {
            return mach_current_rss_mib().unwrap_or(usage.ru_maxrss as f64 / (1024.0 * 1024.0));
        }
    }
    0.0
}

#[cfg(target_os = "macos")]
fn mach_current_rss_mib() -> Option<f64> {
    use std::process::Command;
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kib: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kib / 1024.0)
}

/// Builds the exact 10,000-line Japanese + ANSI benchmark payload from `scripts/bench-terminals.mjs`.
pub fn make_10k_payload() -> (Vec<Vec<u8>>, usize) {
    let mut chunks = Vec::with_capacity(50);
    let mut total_bytes = 0;
    for start in (0..10_000).step_by(200) {
        let mut block = String::with_capacity(20_000);
        for l in start..(start + 200).min(10_000) {
            block.push_str(&format!(
                "\x1b[36m[{l:05}]\x1b[0m \x1b[32mINFO\x1b[0m uiterm-bench: 日本語ログ出力テスト payload=0123456789abcdef\r\n"
            ));
        }
        let bytes = block.into_bytes();
        total_bytes += bytes.len();
        chunks.push(bytes);
    }
    (chunks, total_bytes)
}

pub fn run_headless_vt_benchmark() {
    let (chunks, total_bytes) = make_10k_payload();
    println!(
        "=== Native libghostty-vt Headless Benchmark (120x40, 10,000 lines, {:.1} KiB) ===",
        total_bytes as f64 / 1024.0
    );

    if let Ok(mut w) = VtTerminal::new(120, 40, DEFAULT_SCROLLBACK_LINES) {
        w.feed_raw(&chunks[0]);
    }

    for count in [1usize, 5, 10] {
        let rss0 = current_rss_mib();
        let t_create = Instant::now();
        let mut terms: Vec<VtTerminal> = (0..count)
            .map(|_| VtTerminal::new(120, 40, DEFAULT_SCROLLBACK_LINES).expect("vt new"))
            .collect();
        let create_ms = t_create.elapsed().as_secs_f64() * 1000.0;
        let rss_idle = current_rss_mib();

        let t_write = Instant::now();
        for chunk in &chunks {
            terms[0].feed_raw(chunk);
        }
        let write_ms = t_write.elapsed().as_secs_f64() * 1000.0;
        let mbps = (total_bytes as f64 / (1024.0 * 1024.0)) / (write_ms / 1000.0);

        for term in terms.iter_mut().skip(1) {
            for chunk in &chunks {
                term.feed_raw(chunk);
            }
        }
        let rss_loaded = current_rss_mib();

        let replies = terms[0].feed_raw(b"\x1b[c\x1b[6n");
        let t_snap = Instant::now();
        let snap = terms[0].snapshot().expect("snapshot");
        let snap_ms = t_snap.elapsed().as_secs_f64() * 1000.0;

        let t_drop = Instant::now();
        drop(terms);
        let drop_ms = t_drop.elapsed().as_secs_f64() * 1000.0;
        let rss_after = current_rss_mib();

        println!(
            "count={:<2} create={:5.2}ms write10k={:5.2}ms ({:6.1} MiB/s) snap={:5.3}ms drop={:5.2}ms replies={} grid={}x{} (lines={}) cursor=({},{}) fg=#{:06x} sel={} rss_idle_delta={:+.2}MiB rss_loaded_delta={:+.2}MiB rss_after_drop_delta={:+.2}MiB",
            count,
            create_ms,
            write_ms,
            mbps,
            snap_ms,
            drop_ms,
            replies.len(),
            snap.cols,
            snap.rows,
            snap.lines.len(),
            snap.cursor.col,
            snap.cursor.row,
            snap.colors.fg_u32,
            snap.row_selections.iter().flatten().count(),
            rss_idle - rss0,
            rss_loaded - rss0,
            rss_after - rss0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_faint_and_concealed_backgrounds_and_separates_auto_replies() {
        let mut vt = VtTerminal::new(80, 24, DEFAULT_SCROLLBACK_LINES).expect("vt");
        let events = vt.feed(
            b"\x1b[48;2;64;96;144m\x1b[2KNormal \x1b[2mDIM \x1b[22mnormal \x1b[8mhidden\x1b[28m\x1b[0m\x1b[c",
        );
        assert!(
            !events.pty_writes.is_empty(),
            "expected DA1 automatic response from libghostty-vt"
        );
        let snap = vt.snapshot().expect("snapshot");
        let row0 = &snap.lines[0];
        let dim_cell = row0.iter().find(|c| c.text == "D").expect("DIM cell");
        assert!(dim_cell.faint);
        assert_eq!(dim_cell.bg_rgb, Some((64, 96, 144)));

        let hidden_cell = row0.iter().find(|c| c.text == "h").expect("hidden cell");
        assert!(hidden_cell.invisible);
        assert_eq!(hidden_cell.bg_rgb, Some((64, 96, 144)));
    }

    #[test]
    fn verifies_inverse_color_behavior_and_bracketed_paste_sanitization() {
        let mut vt = VtTerminal::new(80, 24, DEFAULT_SCROLLBACK_LINES).expect("vt");
        vt.feed(b"\x1b[38;2;200;10;10m\x1b[48;2;10;200;10m\x1b[7mX\x1b[0m");
        let snap = vt.snapshot().expect("snapshot");
        let x_cell = snap.lines[0].iter().find(|c| c.text == "X").expect("X");
        assert!(x_cell.inverse);
        // Verify exact RGB values reported by RenderState CellIteration
        assert_eq!(x_cell.fg_rgb, Some((200, 10, 10)));
        assert_eq!(x_cell.bg_rgb, Some((10, 200, 10)));

        // Bracketed paste OFF -> newlines become CR, no 200~ wrapper
        let plain_paste = vt.encode_paste("echo 1\necho 2");
        assert_eq!(plain_paste, b"echo 1\recho 2");

        // Bracketed paste ON -> wrapped in 200~/201~ and ESC stripped
        vt.feed(b"\x1b[?2004h");
        let bp = vt.encode_paste("safe\x1b[201~inject\nline2");
        let bp_str = String::from_utf8_lossy(&bp);
        assert!(bp_str.starts_with("\x1b[200~"));
        assert!(bp_str.ends_with("\x1b[201~"));
        assert!(
            !bp_str[6..bp_str.len() - 6].contains('\x1b'),
            "inner ESC must be sanitized: {bp_str:?}"
        );
    }

    #[test]
    fn dirty_tracking_and_row_id_behave_consistently_across_partial_scroll_and_selection() {
        let mut vt = VtTerminal::new(40, 6, DEFAULT_SCROLLBACK_LINES).expect("vt");
        vt.feed(b"L0\r\nL1\r\nL2\r\nL3\r\nL4\r\nL5");
        vt.render_update(|meta, _snap, _ri, _ci| {
            assert_eq!(meta.dirty, Dirty::Full);
            Ok(())
        })
        .unwrap();

        // No output -> Clean
        vt.render_update(|meta, _snap, _ri, _ci| {
            assert_eq!(meta.dirty, Dirty::Clean);
            Ok(())
        })
        .unwrap();

        // Single row update -> Partial
        vt.feed(b"\rXY");
        let mut dirty_ys = Vec::new();
        vt.render_update(|meta, snap, ri, _ci| {
            assert_eq!(meta.dirty, Dirty::Partial);
            let mut it = ri.update(snap).unwrap();
            while let Some((y, _)) = it.next_dirty() {
                dirty_ys.push(y);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(dirty_ys, vec![5]);

        // Scroll by 1 line: observe row ids and dirty flags
        let mut ids_before = Vec::new();
        vt.render_update(|_meta, snap, ri, _ci| {
            let mut it = ri.update(snap).unwrap();
            while let Some(r) = it.next() {
                ids_before.push(r.id().unwrap());
            }
            Ok(())
        })
        .unwrap();

        vt.feed(b"\r\nL6");
        let mut ids_after = Vec::new();
        let mut dirty_flags = Vec::new();
        vt.render_update(|meta, snap, ri, _ci| {
            // ghostty-vt sets Dirty::Full and per-row dirty=true when viewport rows scroll,
            // even though RowId stays attached to the scrolled physical rows.
            assert_eq!(meta.dirty, Dirty::Full);
            let mut it = ri.update(snap).unwrap();
            while let Some(r) = it.next() {
                ids_after.push(r.id().unwrap());
                dirty_flags.push(r.dirty().unwrap());
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(ids_after[0], ids_before[1]);
        assert!(dirty_flags[5], "newly written bottom row should be dirty");

        // Simultaneous edit of bottom row + scroll keeps RowId of scrolled row unchanged;
        // RowShapeCache therefore validates RowId cache hits via content_hash.
        vt.feed(b"\rEDITED\r\nL7");
        let mut ids_edit_scroll = Vec::new();
        vt.render_update(|meta, snap, ri, _ci| {
            assert_eq!(meta.dirty, Dirty::Full);
            let mut it = ri.update(snap).unwrap();
            while let Some(r) = it.next() {
                ids_edit_scroll.push(r.id().unwrap());
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(ids_edit_scroll[4], ids_after[5]);

        // Scrollback navigation delta exposes scrolled rows with preserved RowIds
        vt.scroll_viewport(ScrollViewport::Delta(-1));
        vt.render_update(|meta, snap, ri, _ci| {
            assert_eq!(meta.dirty, Dirty::Full);
            let mut it = ri.update(snap).unwrap();
            let _ = it.next();
            let r1 = it.next().expect("row 1");
            assert_eq!(r1.id().unwrap(), ids_edit_scroll[0]);
            Ok(())
        })
        .unwrap();
        vt.scroll_viewport(ScrollViewport::Bottom);
        vt.render_update(|_, _, _, _| Ok(())).unwrap();

        // Selection update -> verify RenderState sees selection on row 0 ("L2")
        vt.selection_press(0, 0, 1.0, 1.0);
        vt.selection_drag(2, 0, (20.0, 1.0), false, (8, 0, 120));
        vt.selection_release(2, 0);
        assert_eq!(vt.selected_text().as_deref(), Some("L2"));
        let mut sel_seen = None;
        vt.render_update(|_meta, snap, ri, _ci| {
            let mut it = ri.update(snap).unwrap();
            if let Some(r) = it.next() {
                sel_seen = r.selection().unwrap().map(|s| (s.start_x, s.end_x));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(sel_seen, Some((0, 1)));
    }

    #[test]
    fn osc133_normalizer_adds_redraw_zero_when_omitted() {
        let mut norm = Osc133Normalizer::new();
        let input = b"\x1b]133;A\x07prompt$ \x1b]133;A;redraw=1\x1b\\";
        let out = norm.normalize(input);
        assert_eq!(
            out.as_ref(),
            b"\x1b]133;A;redraw=0\x07prompt$ \x1b]133;A;redraw=1\x1b\\"
        );
    }

    #[test]
    fn key_and_mouse_encoders_produce_valid_vt_sequences() {
        let mut vt = VtTerminal::new(80, 24, DEFAULT_SCROLLBACK_LINES).expect("vt");
        assert_eq!(vt.encode_key(Key::C, Mods::CTRL, None, Some('c')), b"\x03");
        assert_eq!(vt.encode_key(Key::D, Mods::CTRL, None, Some('d')), b"\x04");
        assert_eq!(vt.encode_key(Key::B, Mods::ALT, None, Some('b')), b"\x1bb");
        assert_eq!(
            vt.encode_key(Key::Backspace, Mods::ALT, None, None),
            b"\x1b\x7f"
        );
        assert_eq!(vt.encode_key(Key::Tab, Mods::SHIFT, None, None), b"\x1b[Z");
        assert_eq!(vt.encode_key(Key::ArrowUp, Mods::empty(), None, None), b"\x1b[A");
        vt.feed(b"\x1b[?1h");
        assert_eq!(vt.encode_key(Key::ArrowUp, Mods::empty(), None, None), b"\x1bOA");

        vt.feed(b"\x1b[?1000h\x1b[?1006h");
        assert!(vt.is_mouse_tracking());
        let mouse_bytes = vt.encode_mouse(
            MouseAction::Press,
            Some(MouseButton::Left),
            Mods::empty(),
            10.0,
            20.0,
            true,
            MouseEncoderSize {
                screen_width: 800,
                screen_height: 480,
                cell_width: 8,
                cell_height: 18,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            },
        );
        assert_eq!(mouse_bytes, b"\x1b[<0;2;2M");
    }
}
