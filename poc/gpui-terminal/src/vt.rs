//! Native `libghostty-vt` wrapper and headless throughput benchmark.

use std::time::Instant;

use ghostty_vt::render::{CellIterator, RenderState, RowIterator};
use ghostty_vt::screen::CellWide;
use ghostty_vt::terminal::{Event, Options as TerminalOptions, Terminal};

pub const DEFAULT_SCROLLBACK_MAX_BYTES: usize = 2 * 1024 * 1024; // ~2,000+ lines

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StyledCell {
    pub col: u16,
    pub width: u8,
    pub text: String,
    pub fg_rgb: Option<(u8, u8, u8)>,
    pub bg_rgb: Option<(u8, u8, u8)>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub invisible: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ScreenSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<Vec<StyledCell>>,
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_visible: bool,
}

pub struct VtTerminal {
    terminal: Terminal,
    render_state: RenderState,
    row_iter: RowIterator,
    cell_iter: CellIterator,
    cols: u16,
    rows: u16,
}

impl VtTerminal {
    pub fn new(cols: u16, rows: u16, max_scrollback_bytes: usize) -> Result<Self, String> {
        let terminal = Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: max_scrollback_bytes,
        })
        .map_err(|e| format!("ghostty_vt::Terminal::new: {e}"))?;
        let render_state =
            RenderState::new().map_err(|e| format!("RenderState::new: {e}"))?;
        let row_iter = RowIterator::new().map_err(|e| format!("RowIterator::new: {e}"))?;
        let cell_iter = CellIterator::new().map_err(|e| format!("CellIterator::new: {e}"))?;
        Ok(Self {
            terminal,
            render_state,
            row_iter,
            cell_iter,
            cols,
            rows,
        })
    }

    #[allow(dead_code)]
    pub fn cols(&self) -> u16 {
        self.cols
    }

    #[allow(dead_code)]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Feeds raw PTY bytes into `libghostty-vt` and drains any automatic VT replies
    /// (`Event::PtyWrite`) so the caller can forward them over `FRAME_AUTOMATIC_INPUT`.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.terminal.vt_write(bytes);
        let mut auto_replies = Vec::new();
        for ev in self.terminal.events() {
            if let Event::PtyWrite(reply) = ev {
                auto_replies.push(reply);
            }
        }
        auto_replies
    }

    #[allow(dead_code)]
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_w_px: u32,
        cell_h_px: u32,
    ) -> Result<(), String> {
        if cols == 0 || rows == 0 {
            return Ok(());
        }
        self.terminal
            .resize(cols, rows, cell_w_px, cell_h_px)
            .map_err(|e| format!("Terminal::resize: {e}"))?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    /// Extracts the visible viewport grid using `libghostty-vt`'s `RenderState`.
    pub fn snapshot(&mut self) -> Result<ScreenSnapshot, String> {
        let snap = self
            .render_state
            .update(&self.terminal)
            .map_err(|e| format!("RenderState::update: {e}"))?;
        let cols = snap.cols().unwrap_or(self.cols);
        let rows = snap.rows().unwrap_or(self.rows);
        let cursor = snap.cursor().ok();
        let (cursor_col, cursor_row, cursor_visible) = match cursor {
            Some(c) => (
                c.viewport.as_ref().map(|v| v.x).unwrap_or(0),
                c.viewport.as_ref().map(|v| v.y).unwrap_or(0),
                c.visible && c.viewport.is_some(),
            ),
            None => (0, 0, false),
        };

        let mut lines = Vec::with_capacity(rows as usize);
        let mut grapheme_buf = String::with_capacity(16);

        let mut row_it = self
            .row_iter
            .update(&snap)
            .map_err(|e| format!("RowIterator::update: {e}"))?;
        while let Some(row) = row_it.next() {
            let mut row_cells = Vec::new();
            let mut cell_it = self
                .cell_iter
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
                    let _ = cell.graphemes_utf8(&mut grapheme_buf);
                    let style = cell.style().unwrap_or_default();
                    let fg_rgb = cell.fg_color().ok().flatten().map(|c| (c.r, c.g, c.b));
                    let bg_rgb = cell.bg_color().ok().flatten().map(|c| (c.r, c.g, c.b));
                    if !grapheme_buf.is_empty() || bg_rgb.is_some() || style.inverse {
                        row_cells.push(StyledCell {
                            col,
                            width,
                            text: grapheme_buf.clone(),
                            fg_rgb,
                            bg_rgb,
                            bold: style.bold,
                            italic: style.italic,
                            faint: style.faint,
                            invisible: style.invisible,
                            underline: !matches!(style.underline, ghostty_vt::style::Underline::None),
                            strikethrough: style.strikethrough,
                        });
                    }
                }
                col = col.saturating_add(1);
            }
            lines.push(row_cells);
        }

        let _ = snap.clean();
        Ok(ScreenSnapshot {
            cols,
            rows,
            lines,
            cursor_col,
            cursor_row,
            cursor_visible,
        })
    }
}

/// Returns current process resident set size (RSS) in MiB on macOS.
pub fn current_rss_mib() -> f64 {
    #[cfg(target_os = "macos")]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
        if rc == 0 {
            // On macOS, ru_maxrss is in bytes. For current RSS, use mach task_info.
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

    // Warmup
    if let Ok(mut w) = VtTerminal::new(120, 40, DEFAULT_SCROLLBACK_MAX_BYTES) {
        w.feed(&chunks[0]);
    }

    for count in [1usize, 5, 10] {
        let rss0 = current_rss_mib();
        let t_create = Instant::now();
        let mut terms: Vec<VtTerminal> = (0..count)
            .map(|_| VtTerminal::new(120, 40, DEFAULT_SCROLLBACK_MAX_BYTES).expect("vt new"))
            .collect();
        let create_ms = t_create.elapsed().as_secs_f64() * 1000.0;
        let rss_idle = current_rss_mib();

        let t_write = Instant::now();
        for chunk in &chunks {
            terms[0].feed(chunk);
        }
        let write_ms = t_write.elapsed().as_secs_f64() * 1000.0;
        let mbps = (total_bytes as f64 / (1024.0 * 1024.0)) / (write_ms / 1000.0);

        for term in terms.iter_mut().skip(1) {
            for chunk in &chunks {
                term.feed(chunk);
            }
        }
        let rss_loaded = current_rss_mib();

        let replies = terms[0].feed(b"\x1b[c\x1b[6n");
        let t_snap = Instant::now();
        let snap = terms[0].snapshot().expect("snapshot");
        let snap_ms = t_snap.elapsed().as_secs_f64() * 1000.0;

        let t_drop = Instant::now();
        drop(terms);
        let drop_ms = t_drop.elapsed().as_secs_f64() * 1000.0;
        let rss_after = current_rss_mib();

        println!(
            "count={:<2} create={:5.2}ms write10k={:5.2}ms ({:6.1} MiB/s) snap={:5.3}ms drop={:5.2}ms replies={} rows={} rss_idle_delta={:+.2}MiB rss_loaded_delta={:+.2}MiB rss_after_drop_delta={:+.2}MiB",
            count,
            create_ms,
            write_ms,
            mbps,
            snap_ms,
            drop_ms,
            replies.len(),
            snap.lines.len(),
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
        let mut vt = VtTerminal::new(80, 24, DEFAULT_SCROLLBACK_MAX_BYTES).expect("vt");
        let replies = vt.feed(
            b"\x1b[48;2;64;96;144m\x1b[2KNormal \x1b[2mDIM \x1b[22mnormal \x1b[8mhidden\x1b[28m\x1b[0m\x1b[c",
        );
        assert!(
            !replies.is_empty(),
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
}
