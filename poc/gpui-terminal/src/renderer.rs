//! Terminal grid shaping, `RenderState::next_dirty()` + `RowId` `ShapedLine` cache,
//! and GPUI Metal/Blade quad + glyph painting.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use ghostty_vt::render::{CursorVisualStyle, Dirty, RowId};
use ghostty_vt::screen::Screen;
use ghostty_vt::style::Underline;
use gpui::{
    App, Bounds, Font, Hsla, Pixels, ShapedLine, SharedString, StrikethroughStyle, TextRun,
    UnderlineStyle, Window, WindowTextSystem, fill, hsla, point, px, rgb, rgba, size,
};

use crate::fonts::{DEFAULT_LINE_HEIGHT, FontPreset, build_terminal_font_styled};
use crate::vt::{
    RenderFrameMeta, ScreenColorsSnapshot, StyledCell, VtTerminal, extract_row_cells,
};

const MAX_CACHED_ROWS: usize = 128;
const SELECTION_BG_HSLA: fn() -> Hsla = || hsla(217.0 / 360.0, 0.91, 0.60, 0.30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderCacheMode {
    Uncached,
    DirtyRowCache,
}

impl RenderCacheMode {
    pub fn label(self) -> &'static str {
        match self {
            RenderCacheMode::Uncached => "uncached",
            RenderCacheMode::DirtyRowCache => "cached",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheConfigKey {
    preset: FontPreset,
    nerd_font_fallback: bool,
    font_size_bits: u32,
    line_height_bits: u32,
    scale_factor_bits: u32,
    cols: u16,
    rows: u16,
    active_screen: u8,
    colors_hash: u64,
}

#[derive(Debug, Clone)]
pub struct BgQuad {
    pub col_start: u16,
    pub col_span: u16,
    pub extends_to_right_edge: bool,
    pub rgb_u32: u32,
}

#[derive(Debug, Clone)]
pub struct ShapedSpan {
    pub col_start: u16,
    pub shaped: ShapedLine,
}

#[derive(Debug, Clone)]
pub struct CachedShapedRow {
    pub bg_quads: Vec<BgQuad>,
    pub shaped_spans: Vec<ShapedSpan>,
    pub estimated_bytes: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FrameRenderStats {
    pub prepaint_us: u64,
    pub shape_calls: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub dirty_rows: usize,
    pub cached_entries: usize,
    pub estimated_cache_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct RenderStyleConfig {
    pub preset: FontPreset,
    pub nerd_font_fallback: bool,
    pub font_size: f32,
    pub line_height: f32,
}

impl Default for RenderStyleConfig {
    fn default() -> Self {
        Self {
            preset: FontPreset::Default,
            nerd_font_fallback: true,
            font_size: 14.0,
            line_height: DEFAULT_LINE_HEIGHT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CellMetrics {
    pub cell_width: Pixels,
    pub row_height: Pixels,
}

pub fn measure_cell_metrics(ts: &WindowTextSystem, style: &RenderStyleConfig) -> CellMetrics {
    let font = build_terminal_font_styled(style.preset, style.nerd_font_fallback, false, false);
    let font_id = ts.resolve_font(&font);
    let font_size = px(style.font_size);
    let cell_width = ts
        .advance(font_id, font_size, 'M')
        .map(|s| s.width)
        .unwrap_or(px(8.4));
    let row_height = px((style.font_size * style.line_height).ceil().max(1.0));
    CellMetrics {
        cell_width,
        row_height,
    }
}

pub struct FontVariantSet {
    pub regular: Font,
    pub bold: Font,
    pub italic: Font,
    pub bold_italic: Font,
}

impl FontVariantSet {
    pub fn from_style(style: &RenderStyleConfig) -> Self {
        Self {
            regular: build_terminal_font_styled(
                style.preset,
                style.nerd_font_fallback,
                false,
                false,
            ),
            bold: build_terminal_font_styled(style.preset, style.nerd_font_fallback, true, false),
            italic: build_terminal_font_styled(style.preset, style.nerd_font_fallback, false, true),
            bold_italic: build_terminal_font_styled(
                style.preset,
                style.nerd_font_fallback,
                true,
                true,
            ),
        }
    }

    pub fn select(&self, bold: bool, italic: bool) -> &Font {
        match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }
}

#[inline]
fn fnv1a_mix(mut h: u64, byte: u8) -> u64 {
    h ^= byte as u64;
    h.wrapping_mul(0x1000_0000_01b3)
}

#[inline]
fn fnv1a_u32(mut h: u64, v: u32) -> u64 {
    for b in v.to_le_bytes() {
        h = fnv1a_mix(h, b);
    }
    h
}

pub fn hash_styled_cells(cells: &[StyledCell]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    h = fnv1a_u32(h, cells.len() as u32);
    for c in cells {
        h = fnv1a_u32(h, ((c.col as u32) << 8) | (c.width as u32));
        let flags = (c.bold as u32)
            | ((c.italic as u32) << 1)
            | ((c.faint as u32) << 2)
            | ((c.inverse as u32) << 3)
            | ((c.invisible as u32) << 4)
            | ((c.strikethrough as u32) << 5)
            | ((c.overline as u32) << 6)
            | (((c.underline as u32) & 0xf) << 8);
        h = fnv1a_u32(h, flags);
        if let Some((r, g, b)) = c.fg_rgb {
            h = fnv1a_u32(h, 0x0100_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
        } else {
            h = fnv1a_mix(h, 0);
        }
        if let Some((r, g, b)) = c.bg_rgb {
            h = fnv1a_u32(h, 0x0100_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
        } else {
            h = fnv1a_mix(h, 0);
        }
        if let Some((r, g, b)) = c.underline_rgb {
            h = fnv1a_u32(h, 0x0100_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
        } else {
            h = fnv1a_mix(h, 0);
        }
        for &b in c.text.as_bytes() {
            h = fnv1a_mix(h, b);
        }
        h = fnv1a_mix(h, 0xff);
    }
    h
}

#[derive(Clone)]
struct CachedRowEntry {
    generation: u64,
    content_hash: u64,
    row: Arc<CachedShapedRow>,
}

/// Cross-frame `RowId` + content-hash -> `CachedShapedRow` cache driven by `RenderState::next_dirty()`.
pub struct RowShapeCache {
    pub mode: RenderCacheMode,
    config: Option<CacheConfigKey>,
    viewport_rows: Vec<Option<Arc<CachedShapedRow>>>,
    viewport_selections: Vec<Option<(u16, u16)>>,
    by_row_id: HashMap<RowId, CachedRowEntry>,
    generation: u64,
    last_selection_generation: u64,
    grapheme_buf: String,
    pub last_stats: FrameRenderStats,
}

impl RowShapeCache {
    pub fn new(mode: RenderCacheMode) -> Self {
        Self {
            mode,
            config: None,
            viewport_rows: Vec::new(),
            viewport_selections: Vec::new(),
            by_row_id: HashMap::with_capacity(MAX_CACHED_ROWS),
            generation: 0,
            last_selection_generation: 0,
            grapheme_buf: String::with_capacity(16),
            last_stats: FrameRenderStats::default(),
        }
    }

    pub fn clear(&mut self) {
        self.config = None;
        self.viewport_rows.clear();
        self.viewport_selections.clear();
        self.by_row_id.clear();
        self.last_selection_generation = 0;
    }

    pub fn estimated_bytes(&self) -> usize {
        self.by_row_id
            .values()
            .map(|entry| entry.row.estimated_bytes + std::mem::size_of::<(RowId, CachedRowEntry)>())
            .sum::<usize>()
            + self.viewport_rows.capacity() * std::mem::size_of::<Option<Arc<CachedShapedRow>>>()
    }

    pub fn prepare_frame(
        &mut self,
        vt: &mut VtTerminal,
        style: &RenderStyleConfig,
        scale_factor: f32,
        ts: &WindowTextSystem,
    ) -> Result<PreparedTerminalFrame, String> {
        let t0 = Instant::now();
        let fonts = FontVariantSet::from_style(style);
        let font_size = px(style.font_size);
        let metrics = measure_cell_metrics(ts, style);

        let mut prepared_rows: Vec<Option<Arc<CachedShapedRow>>> = Vec::new();
        let mut prepared_selections: Vec<Option<(u16, u16)>> = Vec::new();
        let mut meta_out: Option<RenderFrameMeta> = None;

        let mut shape_calls = 0usize;
        let mut cache_hits = 0usize;
        let mut cache_misses = 0usize;
        let mut dirty_rows = 0usize;

        let mode = self.mode;
        vt.render_update(|meta, snap, row_iter, cell_iter| {
            meta_out = Some(meta.clone());
            let rows_len = meta.rows as usize;
            let active_screen_id = match meta.active_screen {
                Screen::Primary => 0,
                Screen::Alternate => 1,
            };
            let next_key = CacheConfigKey {
                preset: style.preset,
                nerd_font_fallback: style.nerd_font_fallback,
                font_size_bits: style.font_size.to_bits(),
                line_height_bits: style.line_height.to_bits(),
                scale_factor_bits: scale_factor.to_bits(),
                cols: meta.cols,
                rows: meta.rows,
                active_screen: active_screen_id,
                colors_hash: meta.colors.hash,
            };

            if mode == RenderCacheMode::Uncached {
                let mut row_it = row_iter
                    .update(snap)
                    .map_err(|e| format!("RowIterator::update: {e}"))?;
                prepared_rows.reserve(rows_len);
                prepared_selections.reserve(rows_len);
                while let Some(row) = row_it.next() {
                    dirty_rows += 1;
                    cache_misses += 1;
                    let sel = row.selection().ok().flatten().map(|s| (s.start_x, s.end_x));
                    let cells =
                        extract_row_cells(row, cell_iter, &meta.colors, &mut self.grapheme_buf)?;
                    let (shaped_row, row_shapes) = shape_extracted_row(
                        &cells,
                        meta.cols,
                        &meta.colors,
                        &fonts,
                        font_size,
                        metrics.cell_width,
                        ts,
                    );
                    shape_calls += row_shapes;
                    prepared_rows.push(Some(Arc::new(shaped_row)));
                    prepared_selections.push(sel);
                }
                return Ok(());
            }

            // DirtyRowCache mode
            let config_changed = self.config != Some(next_key) || self.viewport_rows.len() != rows_len;
            if config_changed {
                self.by_row_id.clear();
                self.viewport_rows = vec![None; rows_len];
                self.viewport_selections = vec![None; rows_len];
                self.config = Some(next_key);
            }
            self.generation = self.generation.wrapping_add(1);
            let frame_gen = self.generation;

            if !config_changed && meta.dirty == Dirty::Clean {
                if meta.selection_generation != self.last_selection_generation {
                    let mut row_it = row_iter
                        .update(snap)
                        .map_err(|e| format!("RowIterator::update: {e}"))?;
                    let mut y = 0usize;
                    while let Some(row) = row_it.next() {
                        if y < self.viewport_selections.len() {
                            self.viewport_selections[y] =
                                row.selection().ok().flatten().map(|s| (s.start_x, s.end_x));
                        }
                        y += 1;
                    }
                    self.last_selection_generation = meta.selection_generation;
                }
                cache_hits = rows_len;
            } else if !config_changed && meta.dirty == Dirty::Partial {
                let cache_across_scroll = meta.active_screen == Screen::Primary;
                let mut row_it = row_iter
                    .update(snap)
                    .map_err(|e| format!("RowIterator::update: {e}"))?;
                let mut touched = vec![false; rows_len];
                while let Some((y, row)) = row_it.next_dirty() {
                    let y_idx = y as usize;
                    if y_idx >= rows_len {
                        continue;
                    }
                    touched[y_idx] = true;
                    let sel = row.selection().ok().flatten().map(|s| (s.start_x, s.end_x));
                    let cells =
                        extract_row_cells(row, cell_iter, &meta.colors, &mut self.grapheme_buf)?;

                    if cache_across_scroll {
                        let row_id = row.id().ok();
                        let content_hash = hash_styled_cells(&cells);
                        if let Some(id) = row_id
                            && !id.is_none()
                            && let Some(entry) = self.by_row_id.get_mut(&id)
                            && entry.content_hash == content_hash
                        {
                            entry.generation = frame_gen;
                            self.viewport_rows[y_idx] = Some(entry.row.clone());
                            cache_hits += 1;
                            self.viewport_selections[y_idx] = sel;
                            continue;
                        }
                        dirty_rows += 1;
                        cache_misses += 1;
                        let (shaped_row, row_shapes) = shape_extracted_row(
                            &cells,
                            meta.cols,
                            &meta.colors,
                            &fonts,
                            font_size,
                            metrics.cell_width,
                            ts,
                        );
                        shape_calls += row_shapes;
                        let arc = Arc::new(shaped_row);
                        if let Some(id) = row_id
                            && !id.is_none()
                        {
                            self.by_row_id.insert(
                                id,
                                CachedRowEntry {
                                    generation: frame_gen,
                                    content_hash,
                                    row: arc.clone(),
                                },
                            );
                        }
                        self.viewport_rows[y_idx] = Some(arc);
                    } else {
                        dirty_rows += 1;
                        cache_misses += 1;
                        let (shaped_row, row_shapes) = shape_extracted_row(
                            &cells,
                            meta.cols,
                            &meta.colors,
                            &fonts,
                            font_size,
                            metrics.cell_width,
                            ts,
                        );
                        shape_calls += row_shapes;
                        self.viewport_rows[y_idx] = Some(Arc::new(shaped_row));
                    }
                    self.viewport_selections[y_idx] = sel;
                }
                let untouched = touched.iter().filter(|&&t| !t).count();
                cache_hits += untouched;

                // If selection changed separately on rows not marked dirty, refresh selection spans
                if meta.selection_generation != self.last_selection_generation {
                    let mut row_it2 = row_iter
                        .update(snap)
                        .map_err(|e| format!("RowIterator::update: {e}"))?;
                    let mut y = 0usize;
                    while let Some(row) = row_it2.next() {
                        if y < rows_len && !touched[y] {
                            self.viewport_selections[y] =
                                row.selection().ok().flatten().map(|s| (s.start_x, s.end_x));
                        }
                        y += 1;
                    }
                }
                self.last_selection_generation = meta.selection_generation;
            } else {
                // Full redraw / scroll / config change:
                // Only use the cross-row `RowId + content_hash` cache on Screen::Primary,
                // where scrollback preserves row identity (85-99% hits). Skip it on
                // Screen::Alternate (where apps like Neovim redraw rows in-place on Ctrl+D/U),
                // avoiding hash/eviction overhead and redundant multi-run ShapedLine memory.
                let use_row_id_cache = !config_changed && meta.active_screen == Screen::Primary;
                let mut row_it = row_iter
                    .update(snap)
                    .map_err(|e| format!("RowIterator::update: {e}"))?;
                let mut y = 0usize;
                while let Some(row) = row_it.next() {
                    if y >= rows_len {
                        break;
                    }
                    let sel = row.selection().ok().flatten().map(|s| (s.start_x, s.end_x));
                    self.viewport_selections[y] = sel;

                    let cells = extract_row_cells(
                        row,
                        cell_iter,
                        &meta.colors,
                        &mut self.grapheme_buf,
                    )?;

                    if use_row_id_cache {
                        let row_id = row.id().ok();
                        let content_hash = hash_styled_cells(&cells);
                        if let Some(id) = row_id
                            && !id.is_none()
                            && let Some(entry) = self.by_row_id.get_mut(&id)
                            && entry.content_hash == content_hash
                        {
                            entry.generation = frame_gen;
                            self.viewport_rows[y] = Some(entry.row.clone());
                            cache_hits += 1;
                            y += 1;
                            continue;
                        }
                        dirty_rows += 1;
                        cache_misses += 1;
                        let (shaped_row, row_shapes) = shape_extracted_row(
                            &cells,
                            meta.cols,
                            &meta.colors,
                            &fonts,
                            font_size,
                            metrics.cell_width,
                            ts,
                        );
                        shape_calls += row_shapes;
                        let arc = Arc::new(shaped_row);
                        if let Some(id) = row_id
                            && !id.is_none()
                        {
                            self.by_row_id.insert(
                                id,
                                CachedRowEntry {
                                    generation: frame_gen,
                                    content_hash,
                                    row: arc.clone(),
                                },
                            );
                        }
                        self.viewport_rows[y] = Some(arc);
                    } else {
                        dirty_rows += 1;
                        cache_misses += 1;
                        let (shaped_row, row_shapes) = shape_extracted_row(
                            &cells,
                            meta.cols,
                            &meta.colors,
                            &fonts,
                            font_size,
                            metrics.cell_width,
                            ts,
                        );
                        shape_calls += row_shapes;
                        self.viewport_rows[y] = Some(Arc::new(shaped_row));
                    }
                    y += 1;
                }
                self.last_selection_generation = meta.selection_generation;
            }

            if self.by_row_id.len() > MAX_CACHED_ROWS {
                self.evict_old_entries();
            }

            prepared_rows = self.viewport_rows.clone();
            prepared_selections = self.viewport_selections.clone();
            Ok(())
        })?;

        let meta = meta_out.expect("meta populated");
        let cached_entries = if self.mode == RenderCacheMode::DirtyRowCache {
            self.by_row_id.len()
        } else {
            0
        };
        let estimated_cache_bytes = if self.mode == RenderCacheMode::DirtyRowCache {
            self.estimated_bytes()
        } else {
            0
        };

        self.last_stats = FrameRenderStats {
            prepaint_us: t0.elapsed().as_micros() as u64,
            shape_calls,
            cache_hits,
            cache_misses,
            dirty_rows,
            cached_entries,
            estimated_cache_bytes,
        };

        Ok(PreparedTerminalFrame {
            metrics,
            meta,
            rows: prepared_rows,
            selections: prepared_selections,
        })
    }

    fn evict_old_entries(&mut self) {
        if self.by_row_id.len() <= MAX_CACHED_ROWS {
            return;
        }
        let mut entries: Vec<(RowId, u64)> = self
            .by_row_id
            .iter()
            .map(|(id, entry)| (*id, entry.generation))
            .collect();
        entries.sort_by_key(|&(_, frame_gen)| frame_gen);
        let to_remove = entries.len().saturating_sub(MAX_CACHED_ROWS);
        for (id, _) in entries.into_iter().take(to_remove) {
            self.by_row_id.remove(&id);
        }
    }
}

pub struct PreparedTerminalFrame {
    pub metrics: CellMetrics,
    pub meta: RenderFrameMeta,
    pub rows: Vec<Option<Arc<CachedShapedRow>>>,
    pub selections: Vec<Option<(u16, u16)>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RunStyleKey {
    bold: bool,
    italic: bool,
    faint: bool,
    eff_fg: u32,
    underline: u8,
    underline_rgb: Option<(u8, u8, u8)>,
    strikethrough: bool,
}

fn push_bg_quad(bg_quads: &mut Vec<BgQuad>, col: u16, span_w: u16, bg_u32: u32) {
    if let Some(last) = bg_quads.last_mut()
        && last.rgb_u32 == bg_u32
        && last.col_start + last.col_span == col
    {
        last.col_span += span_w;
    } else {
        bg_quads.push(BgQuad {
            col_start: col,
            col_span: span_w,
            extends_to_right_edge: false,
            rgb_u32: bg_u32,
        });
    }
}

fn make_text_run(
    cell: &StyledCell,
    text_len: usize,
    eff_fg: u32,
    fonts: &FontVariantSet,
) -> (TextRun, RunStyleKey) {
    let fg_hsla: Hsla = if cell.faint {
        rgba((eff_fg << 8) | 0x80).into()
    } else {
        rgb(eff_fg).into()
    };
    let underline = underline_style_for(cell, eff_fg);
    let strikethrough = if cell.strikethrough {
        Some(StrikethroughStyle {
            thickness: px(1.0),
            color: Some(fg_hsla),
        })
    } else {
        None
    };
    let run = TextRun {
        len: text_len,
        font: fonts.select(cell.bold, cell.italic).clone(),
        color: fg_hsla,
        background_color: None,
        underline,
        strikethrough,
    };
    let style_key = RunStyleKey {
        bold: cell.bold,
        italic: cell.italic,
        faint: cell.faint,
        eff_fg,
        underline: cell.underline as u8,
        underline_rgb: cell.underline_rgb,
        strikethrough: cell.strikethrough,
    };
    (run, style_key)
}

pub fn is_coalesceable_bmp_cjk(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(ch) = chars.next() else {
        return false;
    };
    if chars.next().is_some() {
        return false;
    }
    matches!(
        ch as u32,
        0x3000..=0x30FF | 0x3400..=0x9FFF | 0xF900..=0xFAFF | 0xFF01..=0xFF60
    )
}

fn push_coalesced_run(
    runs: &mut Vec<(TextRun, RunStyleKey)>,
    cell: &StyledCell,
    eff_fg: u32,
    fonts: &FontVariantSet,
) {
    let (run, style_key) = make_text_run(cell, cell.text.len(), eff_fg, fonts);
    if let Some((prev_run, prev_key)) = runs.last_mut()
        && *prev_key == style_key
    {
        prev_run.len += run.len;
    } else {
        runs.push((run, style_key));
    }
}

pub fn shape_extracted_row(
    cells: &[StyledCell],
    total_cols: u16,
    colors: &ScreenColorsSnapshot,
    fonts: &FontVariantSet,
    font_size: Pixels,
    cell_width: Pixels,
    ts: &WindowTextSystem,
) -> (CachedShapedRow, usize) {
    let mut bg_quads: Vec<BgQuad> = Vec::new();
    let mut shaped_spans: Vec<ShapedSpan> = Vec::new();
    let mut shape_calls = 0usize;

    let mut idx = 0usize;
    while idx < cells.len() {
        let cell = &cells[idx];
        let (eff_fg, eff_bg) = resolve_cell_colors(cell, colors);

        // Background quad (including SGR 2 faint & SGR 8 invisible & SGR 7 inverse)
        if let Some(bg_u32) = eff_bg {
            push_bg_quad(&mut bg_quads, cell.col, cell.width.max(1) as u16, bg_u32);
        }

        // Foreground glyphs (skip SGR 8 invisible and empty text)
        if !cell.invisible && !cell.text.is_empty() {
            let start_col = cell.col;

            if cell.width == 1 && cell.text.is_ascii() {
                // Coalesce contiguous ASCII cells across color/attribute changes into a single
                // GPUI `ShapedLine` with multiple `TextRun`s (`SmallVec<[DecorationRun; 32]>`).
                let mut text_buf = cell.text.clone();
                let mut runs: Vec<(TextRun, RunStyleKey)> = Vec::with_capacity(8);
                push_coalesced_run(&mut runs, cell, eff_fg, fonts);
                let mut expected_col = cell.col + 1;

                while idx + 1 < cells.len() {
                    let next = &cells[idx + 1];
                    if next.col == expected_col
                        && next.width == 1
                        && next.text.is_ascii()
                        && !next.invisible
                        && !next.text.is_empty()
                    {
                        let (next_fg, next_bg) = resolve_cell_colors(next, colors);
                        if let Some(nbg) = next_bg {
                            push_bg_quad(&mut bg_quads, next.col, 1, nbg);
                        }
                        text_buf.push_str(&next.text);
                        push_coalesced_run(&mut runs, next, next_fg, fonts);
                        expected_col += 1;
                        idx += 1;
                    } else {
                        break;
                    }
                }

                let text_runs: Vec<TextRun> = runs.into_iter().map(|(r, _)| r).collect();
                let shaped = ts.shape_line(
                    SharedString::from(text_buf),
                    font_size,
                    &text_runs,
                    Some(cell_width),
                );
                shape_calls += 1;
                shaped_spans.push(ShapedSpan {
                    col_start: start_col,
                    shaped,
                });
            } else if cell.width == 2 && is_coalesceable_bmp_cjk(&cell.text) {
                // Coalesce contiguous BMP CJK wide glyphs across style changes into one `ShapedLine`
                // snapped to exact `2 * cell_width` columns; keep emoji/ZWJ/combining/PUA isolated.
                let mut text_buf = cell.text.clone();
                let mut runs: Vec<(TextRun, RunStyleKey)> = Vec::with_capacity(4);
                push_coalesced_run(&mut runs, cell, eff_fg, fonts);
                let mut expected_col = cell.col + 2;

                while idx + 1 < cells.len() {
                    let next = &cells[idx + 1];
                    if next.col == expected_col
                        && next.width == 2
                        && is_coalesceable_bmp_cjk(&next.text)
                        && !next.invisible
                    {
                        let (next_fg, next_bg) = resolve_cell_colors(next, colors);
                        if let Some(nbg) = next_bg {
                            push_bg_quad(&mut bg_quads, next.col, 2, nbg);
                        }
                        text_buf.push_str(&next.text);
                        push_coalesced_run(&mut runs, next, next_fg, fonts);
                        expected_col += 2;
                        idx += 1;
                    } else {
                        break;
                    }
                }

                let text_runs: Vec<TextRun> = runs.into_iter().map(|(r, _)| r).collect();
                let shaped = ts.shape_line(
                    SharedString::from(text_buf),
                    font_size,
                    &text_runs,
                    Some(cell_width * 2.0),
                );
                shape_calls += 1;
                shaped_spans.push(ShapedSpan {
                    col_start: start_col,
                    shaped,
                });
            } else {
                let (run, _) = make_text_run(cell, cell.text.len(), eff_fg, fonts);
                let shaped = ts.shape_line(
                    SharedString::from(cell.text.clone()),
                    font_size,
                    &[run],
                    None,
                );
                shape_calls += 1;
                shaped_spans.push(ShapedSpan {
                    col_start: start_col,
                    shaped,
                });
            }
        }

        idx += 1;
    }

    // Extend a uniform full-row background to the right viewport edge (matches Phase 2 fix).
    if bg_quads.len() == 1
        && total_cols > 0
        && bg_quads[0].col_start == 0
        && bg_quads[0].col_span >= total_cols
    {
        bg_quads[0].extends_to_right_edge = true;
    }

    let estimated_bytes = std::mem::size_of::<CachedShapedRow>()
        + bg_quads.capacity() * std::mem::size_of::<BgQuad>()
        + shaped_spans
            .iter()
            .map(|s| {
                std::mem::size_of::<ShapedSpan>()
                    + s.shaped.text.len()
                    + s.shaped.runs.len() * 64
                    + 64
            })
            .sum::<usize>();

    (
        CachedShapedRow {
            bg_quads,
            shaped_spans,
            estimated_bytes,
        },
        shape_calls,
    )
}

fn resolve_cell_colors(cell: &StyledCell, colors: &ScreenColorsSnapshot) -> (u32, Option<u32>) {
    let raw_fg = cell
        .fg_rgb
        .map(|(r, g, b)| ((r as u32) << 16) | ((g as u32) << 8) | (b as u32));
    let raw_bg = cell
        .bg_rgb
        .map(|(r, g, b)| ((r as u32) << 16) | ((g as u32) << 8) | (b as u32));

    if cell.inverse {
        let fg = raw_bg.unwrap_or(colors.bg_u32);
        let bg = raw_fg.unwrap_or(colors.fg_u32);
        (fg, Some(bg))
    } else {
        (raw_fg.unwrap_or(colors.fg_u32), raw_bg)
    }
}

fn underline_style_for(cell: &StyledCell, eff_fg_u32: u32) -> Option<UnderlineStyle> {
    if matches!(cell.underline, Underline::None) {
        return None;
    }
    let u_rgb = cell
        .underline_rgb
        .map(|(r, g, b)| ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
        .unwrap_or(eff_fg_u32);
    let color: Hsla = if cell.faint {
        rgba((u_rgb << 8) | 0x80).into()
    } else {
        rgb(u_rgb).into()
    };
    Some(UnderlineStyle {
        thickness: if matches!(cell.underline, Underline::Double) {
            px(2.0)
        } else {
            px(1.0)
        },
        color: Some(color),
        wavy: matches!(cell.underline, Underline::Curly),
    })
}

pub struct ImeOverlayState<'a> {
    pub preedit: &'a str,
    pub marked_range: Option<std::ops::Range<usize>>,
    pub selected_range: std::ops::Range<usize>,
    pub font: Font,
    pub font_size: f32,
}

pub fn paint_terminal_frame(
    frame: &PreparedTerminalFrame,
    content_bounds: Bounds<Pixels>,
    window_active: bool,
    cursor_blink_on: bool,
    ime: Option<ImeOverlayState<'_>>,
    window: &mut Window,
    cx: &mut App,
) -> Option<(Bounds<Pixels>, ShapedLine)> {
    let origin = content_bounds.origin;
    let cw = frame.metrics.cell_width;
    let rh = frame.metrics.row_height;

    // 1. Default terminal background
    window.paint_quad(fill(content_bounds, rgb(frame.meta.colors.bg_u32)));

    // 2. Per-row cell backgrounds + selection highlights + text runs
    for (row_idx, row_opt) in frame.rows.iter().enumerate() {
        let y = origin.y + rh * row_idx as f32;

        if let Some(row) = row_opt {
            for q in &row.bg_quads {
                let x = origin.x + cw * q.col_start as f32;
                let w = if q.extends_to_right_edge {
                    (content_bounds.right() - x).max(cw * q.col_span as f32)
                } else {
                    cw * q.col_span as f32
                };
                window.paint_quad(fill(Bounds::new(point(x, y), size(w, rh)), rgb(q.rgb_u32)));
            }
        }

        if let Some(Some((start_x, end_x))) = frame.selections.get(row_idx)
            && end_x >= start_x
        {
            let x = origin.x + cw * (*start_x as f32);
            let cols_sel = (end_x - start_x + 1) as f32;
            let w = cw * cols_sel;
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(w, rh)),
                SELECTION_BG_HSLA(),
            ));
        }

        if let Some(row) = row_opt {
            for span in &row.shaped_spans {
                let x = origin.x + cw * span.col_start as f32;
                let _ = span.shaped.paint(point(x, y), rh, window, cx);
            }
        }
    }

    // 3. Scrollback indicator when scrolled into history
    if let Some(sb) = frame.meta.scrollbar
        && sb.total > sb.len
        && sb.len > 0
    {
        let track_h = f32::from(content_bounds.size.height);
        let thumb_h = ((sb.len as f32 / sb.total as f32) * track_h).max(18.0).min(track_h);
        let max_offset = (sb.total - sb.len).max(1) as f32;
        let ratio = (sb.offset as f32 / max_offset).clamp(0.0, 1.0);
        let thumb_y = f32::from(content_bounds.top()) + ratio * (track_h - thumb_h);
        window.paint_quad(fill(
            Bounds::new(
                point(content_bounds.right() - px(4.0), px(thumb_y)),
                size(px(3.0), px(thumb_h)),
            ),
            hsla(0.0, 0.0, 0.7, 0.45),
        ));
    }

    // 4. Cursor
    let cursor = frame.meta.cursor;
    let cursor_color_u32 = frame.meta.colors.cursor_u32.unwrap_or(0xe4e4e7);
    if cursor.visible {
        let show_filled = !cursor.blinking || cursor_blink_on;
        let cursor_col = if cursor.at_wide_tail && cursor.col > 0 {
            cursor.col - 1
        } else {
            cursor.col
        };
        let cx_pos = point(origin.x + cw * cursor_col as f32, origin.y + rh * cursor.row as f32);
        let cw_cells = if cursor.at_wide_tail { cw * 2.0 } else { cw };

        if !window_active || matches!(cursor.visual_style, CursorVisualStyle::BlockHollow) {
            window.paint_quad(gpui::outline(
                Bounds::new(cx_pos, size(cw_cells, rh)),
                rgb(cursor_color_u32),
                Default::default(),
            ));
        } else if show_filled {
            match cursor.visual_style {
                CursorVisualStyle::Block => {
                    window.paint_quad(fill(
                        Bounds::new(cx_pos, size(cw_cells, rh)),
                        hsla(0.0, 0.0, 0.92, 0.45),
                    ));
                }
                CursorVisualStyle::Bar => {
                    window.paint_quad(fill(
                        Bounds::new(cx_pos, size(px(2.0), rh)),
                        rgb(cursor_color_u32),
                    ));
                }
                CursorVisualStyle::Underline => {
                    window.paint_quad(fill(
                        Bounds::new(point(cx_pos.x, cx_pos.y + rh - px(2.0)), size(cw_cells, px(2.0))),
                        rgb(cursor_color_u32),
                    ));
                }
                CursorVisualStyle::BlockHollow | _ => {
                    window.paint_quad(gpui::outline(
                        Bounds::new(cx_pos, size(cw_cells, rh)),
                        rgb(cursor_color_u32),
                        Default::default(),
                    ));
                }
            }
        }
    }

    // 5. IME preedit overlay & candidate anchor bounds anchored at cursor position
    let cursor_pos = point(
        origin.x + cw * cursor.col as f32,
        origin.y + rh * cursor.row as f32,
    );
    let mut ime_anchor = Bounds::new(cursor_pos, size(cw.max(px(2.0)), rh));

    if let Some(ime_state) = ime
        && !ime_state.preedit.is_empty()
    {
        let mut runs = Vec::new();
        let preedit = ime_state.preedit;
        if let Some(mark) = &ime_state.marked_range {
            if mark.start > 0 {
                runs.push(TextRun {
                    len: mark.start,
                    font: ime_state.font.clone(),
                    color: rgb(frame.meta.colors.fg_u32).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
            if !mark.is_empty() {
                runs.push(TextRun {
                    len: mark.end - mark.start,
                    font: ime_state.font.clone(),
                    color: rgb(0xeab308).into(),
                    background_color: None,
                    underline: Some(UnderlineStyle {
                        thickness: px(1.5),
                        color: Some(rgb(0xeab308).into()),
                        wavy: false,
                    }),
                    strikethrough: None,
                });
            }
            if mark.end < preedit.len() {
                runs.push(TextRun {
                    len: preedit.len() - mark.end,
                    font: ime_state.font.clone(),
                    color: rgb(frame.meta.colors.fg_u32).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
        } else {
            runs.push(TextRun {
                len: preedit.len(),
                font: ime_state.font.clone(),
                color: rgb(0xeab308).into(),
                background_color: None,
                underline: Some(UnderlineStyle {
                    thickness: px(1.5),
                    color: Some(rgb(0xeab308).into()),
                    wavy: false,
                }),
                strikethrough: None,
            });
        }

        let shaped_preedit = window.text_system().shape_line(
            SharedString::from(preedit.to_string()),
            px(ime_state.font_size),
            &runs,
            None,
        );
        // Clamp draw origin horizontally if preedit extends past right edge so candidate box stays visible
        let max_left = (content_bounds.right() - shaped_preedit.width - px(4.0)).max(origin.x);
        let draw_x = cursor_pos.x.min(max_left);
        let draw_origin = point(draw_x, cursor_pos.y);

        let overlay_bounds = Bounds::new(draw_origin, size(shaped_preedit.width.max(cw), rh));
        window.paint_quad(fill(overlay_bounds, rgb(0x18181b)));
        window.paint_quad(fill(overlay_bounds, hsla(0.13, 0.85, 0.5, 0.16)));
        let _ = shaped_preedit.paint(draw_origin, rh, window, cx);

        let sel_x = shaped_preedit.x_for_index(ime_state.selected_range.end.min(preedit.len()));
        window.paint_quad(fill(
            Bounds::new(point(draw_origin.x + sel_x, draw_origin.y), size(px(2.0), rh)),
            rgb(0x22c55e),
        ));

        ime_anchor = overlay_bounds;
        return Some((ime_anchor, shaped_preedit));
    }

    let empty_shaped = ShapedLine::default();
    Some((ime_anchor, empty_shaped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::DEFAULT_SCROLLBACK_LINES;

    #[test]
    fn resolves_sgr2_faint_sgr8_invisible_and_sgr7_inverse_backgrounds() {
        let colors = ScreenColorsSnapshot {
            fg_u32: 0xf4f4f5,
            bg_u32: 0x09090b,
            cursor_u32: Some(0xe4e4e7),
            palette: ghostty_vt::style::Palette::default().0,
            hash: 1,
        };

        let dim = StyledCell {
            col: 0,
            width: 1,
            text: "D".into(),
            fg_rgb: None,
            bg_rgb: Some((64, 96, 144)),
            bold: false,
            italic: false,
            faint: true,
            inverse: false,
            invisible: false,
            underline: Underline::None,
            underline_rgb: None,
            strikethrough: false,
            overline: false,
        };
        assert_eq!(resolve_cell_colors(&dim, &colors), (0xf4f4f5, Some(0x406090)));

        let hidden = StyledCell {
            invisible: true,
            ..dim.clone()
        };
        assert_eq!(
            resolve_cell_colors(&hidden, &colors),
            (0xf4f4f5, Some(0x406090))
        );

        let inv_default = StyledCell {
            col: 1,
            width: 1,
            text: "I".into(),
            fg_rgb: None,
            bg_rgb: None,
            bold: false,
            italic: false,
            faint: false,
            inverse: true,
            invisible: false,
            underline: Underline::None,
            underline_rgb: None,
            strikethrough: false,
            overline: false,
        };
        assert_eq!(
            resolve_cell_colors(&inv_default, &colors),
            (0x09090b, Some(0xf4f4f5))
        );
    }

    #[test]
    fn cache_eviction_bounds_memory_and_clears_on_config_change() {
        let mut cache = RowShapeCache::new(RenderCacheMode::DirtyRowCache);
        for i in 0..320u64 {
            cache.by_row_id.insert(
                RowId([i + 1, 0]),
                CachedRowEntry {
                    generation: i,
                    content_hash: i + 1000,
                    row: Arc::new(CachedShapedRow {
                        bg_quads: Vec::new(),
                        shaped_spans: Vec::new(),
                        estimated_bytes: 128,
                    }),
                },
            );
        }
        cache.evict_old_entries();
        assert_eq!(cache.by_row_id.len(), MAX_CACHED_ROWS);
        assert!(!cache.by_row_id.contains_key(&RowId([1, 0])));
        assert!(cache.by_row_id.contains_key(&RowId([320, 0])));

        // Clear drops everything cleanly
        cache.clear();
        assert!(cache.by_row_id.is_empty());

        // Sanity check VtTerminal render_update & content_hash invalidation across edit+scroll
        let mut vt = VtTerminal::new(40, 4, DEFAULT_SCROLLBACK_LINES).unwrap();
        vt.feed(b"ROW0\r\nROW1\r\nROW2\r\nROW3");
        let mut row3_id = None;
        let mut row3_hash = 0u64;
        let mut row2_hash = 0u64;
        let mut buf = String::new();
        vt.render_update(|_, snap, ri, ci| {
            let mut it = ri.update(snap).unwrap();
            let mut y = 0;
            while let Some(r) = it.next() {
                let cells = extract_row_cells(r, ci, &ScreenColorsSnapshot::from_snapshot(snap), &mut buf)?;
                let h = hash_styled_cells(&cells);
                if y == 2 {
                    row2_hash = h;
                } else if y == 3 {
                    row3_id = r.id().ok();
                    row3_hash = h;
                }
                y += 1;
            }
            Ok(())
        })
        .unwrap();

        vt.feed(b"\rEDITED\r\nROW4");
        vt.render_update(|meta, snap, ri, ci| {
            let mut it = ri.update(snap).unwrap();
            let mut y = 0;
            while let Some(r) = it.next() {
                let cells = extract_row_cells(r, ci, &meta.colors, &mut buf)?;
                let h = hash_styled_cells(&cells);
                if y == 1 {
                    assert_eq!(h, row2_hash, "unmodified scrolled row must keep identical content hash");
                } else if y == 2 {
                    assert_eq!(r.id().ok(), row3_id);
                    assert_ne!(h, row3_hash, "edited-then-scrolled row must invalidate content hash");
                }
                y += 1;
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn bmp_cjk_coalescing_excludes_emoji_pua_and_combining_marks() {
        assert!(is_coalesceable_bmp_cjk("あ"));
        assert!(is_coalesceable_bmp_cjk("漢"));
        assert!(is_coalesceable_bmp_cjk("Ａ"));
        assert!(!is_coalesceable_bmp_cjk("🦀"));
        assert!(!is_coalesceable_bmp_cjk("👨‍👩‍👧‍👦"));
        assert!(!is_coalesceable_bmp_cjk("か\u{3099}"));
        assert!(!is_coalesceable_bmp_cjk("\u{E0A0}"));
        assert!(!is_coalesceable_bmp_cjk("\u{F024B}"));
        assert!(!is_coalesceable_bmp_cjk("ｱ"));
    }
}
