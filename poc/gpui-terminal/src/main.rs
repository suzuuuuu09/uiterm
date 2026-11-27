mod fonts;
mod herdr;
mod ime;
mod vt;

use std::ops::Range;
use std::time::{Duration, Instant};

use fonts::{
    DEFAULT_FONT_SIZE, DEFAULT_LINE_HEIGHT, FontPreset, MAX_FONT_SIZE, MIN_FONT_SIZE,
    build_terminal_font, layout_cells, string_cell_width,
};
use gpui::{
    App, Application, Bounds, Context, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, Font, GlobalElementId, InspectorElementId,
    IntoElement, KeyBinding, LayoutId, MouseButton, PaintQuad, Pixels, Point, Render, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, WindowBounds,
    WindowOptions, actions, div, fill, hsla, point, prelude::*, px, rgb, rgba, size,
};
use herdr::{IsolatedHerdrSession, SessionOutput};
use ime::ImeBuffer;
use vt::{DEFAULT_SCROLLBACK_MAX_BYTES, ScreenSnapshot, VtTerminal, current_rss_mib};

actions!(
    gpui_poc,
    [
        ImeBackspace,
        ImeEnter,
        ImeEscape,
        FontZoomIn,
        FontZoomOut,
        FontZoomReset,
        QuitPoc
    ]
);

const BG_PRIMARY: u32 = 0x09090b;
const BG_SURFACE: u32 = 0x18181b;
const BORDER_SUBTLE: u32 = 0x27272a;
const FG_PRIMARY: u32 = 0xf4f4f5;
const FG_MUTED: u32 = 0xa1a1aa;
const ACCENT_BLUE: u32 = 0x3b82f6;
const ACCENT_GREEN: u32 = 0x22c55e;
const ACCENT_YELLOW: u32 = 0xeab308;
const ACCENT_CYAN: u32 = 0x06b6d4;
const FAINT_BG: u32 = 0x406090;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Interactive,
    ProbeFonts,
    BenchWindow,
}

struct ProbeCase {
    label: &'static str,
    sample: &'static str,
}

const PROBE_CASES: &[ProbeCase] = &[
    ProbeCase {
        label: "ASCII 40 cols",
        sample: "0123456789012345678901234567890123456789|",
    },
    ProbeCase {
        label: "CJK 20 chars",
        sample: "あいうえおかきくけこさしすせそたちつてと|",
    },
    ProbeCase {
        label: "Half-width kana",
        sample: "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘ|",
    },
    ProbeCase {
        label: "NFD combining",
        sample: "か\u{3099}き\u{3099}く\u{3099}け\u{3099}こ\u{3099} は\u{309A}ひ\u{309A}ふ\u{309A}へ\u{309A}ほ\u{309A}              |",
    },
    ProbeCase {
        label: "Emoji & ZWJ",
        sample: "🦀 🚀 ✨ 🎉 👨‍👩‍👧‍👦 🇯🇵                          |",
    },
    ProbeCase {
        label: "Nerd Font PUA",
        sample: "\u{E0B0} \u{E0B2} \u{E0A0} main \u{E718} \u{F015} \u{F024B} Powerline/Git        |",
    },
];

struct BenchState {
    started_at: Instant,
    first_paint_ms: Option<f64>,
    herdr_rt: tokio::runtime::Runtime,
    herdr_session: Option<IsolatedHerdrSession>,
    vt: VtTerminal,
    snapshot: Option<ScreenSnapshot>,
    frame_times_ms: Vec<f64>,
    last_frame_at: Option<Instant>,
    total_frames: usize,
    seq_roundtrip_ms: f64,
    seq_bytes: usize,
}

struct GpuiPocView {
    focus_handle: FocusHandle,
    preset: FontPreset,
    nerd_font_fallback: bool,
    font_size: f32,
    ime: ImeBuffer,
    last_ime_bounds: Option<Bounds<Pixels>>,
    last_ime_line: Option<ShapedLine>,
    mode: RunMode,
    probe_done: bool,
    window_active: bool,
    bench: Option<BenchState>,
}

impl GpuiPocView {
    fn new(mode: RunMode, started_at: Instant, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            if this.window_active != active {
                this.window_active = active;
                if active {
                    this.ime.on_window_focus();
                } else {
                    this.ime.on_window_blur();
                }
                cx.notify();
            }
        })
        .detach();

        let bench = if mode == RunMode::BenchWindow {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            let mut vt = VtTerminal::new(106, 32, DEFAULT_SCROLLBACK_MAX_BYTES).expect("vt new");

            // Start isolated herdr-core v0.1.22 and measure real PTY `seq 1 10000` round-trip
            let mut session = rt
                .block_on(IsolatedHerdrSession::start(
                    "bench",
                    Some(vec!["/bin/sh".into()]),
                    106,
                    32,
                ))
                .expect("start isolated herdr-core");

            let t_seq = Instant::now();
            rt.block_on(
                session.write_user(b"seq 1 10000; printf '%s%s\\n' UITERM_GPUI_ BENCH_DONE\n".to_vec()),
            )
            .expect("write seq");
            let mut seq_bytes = 0usize;
            let mut saw_done = false;
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && !saw_done {
                let maybe = rt.block_on(async {
                    tokio::time::timeout(Duration::from_millis(200), session.recv_output()).await
                });
                if let Ok(Some(SessionOutput::Data(bytes))) = maybe {
                    seq_bytes += bytes.len();
                    if bytes
                        .windows(b"UITERM_GPUI_BENCH_DONE".len())
                        .any(|w| w == b"UITERM_GPUI_BENCH_DONE")
                    {
                        saw_done = true;
                    }
                    let auto_replies = vt.feed(&bytes);
                    for reply in auto_replies {
                        let _ = rt.block_on(session.write_automatic(reply));
                    }
                }
            }
            let seq_roundtrip_ms = t_seq.elapsed().as_secs_f64() * 1000.0;
            let snapshot = vt.snapshot().ok();

            Some(BenchState {
                started_at,
                first_paint_ms: None,
                herdr_rt: rt,
                herdr_session: Some(session),
                vt,
                snapshot,
                frame_times_ms: Vec::with_capacity(360),
                last_frame_at: None,
                total_frames: 0,
                seq_roundtrip_ms,
                seq_bytes,
            })
        } else {
            None
        };

        Self {
            focus_handle,
            preset: FontPreset::Default,
            nerd_font_fallback: true,
            font_size: DEFAULT_FONT_SIZE,
            ime: ImeBuffer::new(),
            last_ime_bounds: None,
            last_ime_line: None,
            mode,
            probe_done: false,
            window_active: true,
            bench,
        }
    }

    fn current_font(&self) -> Font {
        build_terminal_font(self.preset, self.nerd_font_fallback)
    }

    fn on_backspace(&mut self, _: &ImeBackspace, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime.backspace();
        cx.notify();
    }

    fn on_enter(&mut self, _: &ImeEnter, _window: &mut Window, cx: &mut Context<Self>) {
        if self.ime.submit_line() {
            cx.notify();
        }
    }

    fn on_escape(&mut self, _: &ImeEscape, _window: &mut Window, cx: &mut Context<Self>) {
        if self.ime.cancel_mark_if_any() {
            cx.notify();
        }
    }

    fn on_zoom_in(&mut self, _: &FontZoomIn, _window: &mut Window, cx: &mut Context<Self>) {
        self.font_size = (self.font_size + 1.0).min(MAX_FONT_SIZE);
        cx.notify();
    }

    fn on_zoom_out(&mut self, _: &FontZoomOut, _window: &mut Window, cx: &mut Context<Self>) {
        self.font_size = (self.font_size - 1.0).max(MIN_FONT_SIZE);
        cx.notify();
    }

    fn on_zoom_reset(&mut self, _: &FontZoomReset, _window: &mut Window, cx: &mut Context<Self>) {
        self.font_size = DEFAULT_FONT_SIZE;
        cx.notify();
    }

    fn step_bench_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.as_mut() else {
            return;
        };
        let now = Instant::now();
        if bench.first_paint_ms.is_none() {
            bench.first_paint_ms = Some(now.duration_since(bench.started_at).as_secs_f64() * 1000.0);
        }
        if let Some(prev) = bench.last_frame_at {
            bench
                .frame_times_ms
                .push(now.duration_since(prev).as_secs_f64() * 1000.0);
        }
        bench.last_frame_at = Some(now);

        // Simulate active 360-frame scroll/redraw workload (106x32 viewport, 1 line scroll/frame)
        let frame_idx = bench.total_frames;
        let line = format!(
            "\x1b[36m[{frame_idx:05}]\x1b[0m \x1b[32mINFO\x1b[0m \u{E0A0} main 日本語スクロール描画テスト Neovim/Ctrl+D frame={frame_idx}\r\n"
        );
        bench.vt.feed(line.as_bytes());
        bench.snapshot = bench.vt.snapshot().ok();
        bench.total_frames += 1;

        if bench.frame_times_ms.len() >= 360 {
            let mut sorted = bench.frame_times_ms.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let p50 = sorted[sorted.len() / 2];
            let p95 = sorted[(sorted.len() * 95) / 100];
            let max_ms = *sorted.last().unwrap_or(&0.0);
            let over_16 = sorted.iter().filter(|&&t| t > 16.67).count();
            let over_25 = sorted.iter().filter(|&&t| t > 25.0).count();
            let main_rss = current_rss_mib();
            let core_pid = bench
                .herdr_session
                .as_ref()
                .map(|s| s.core_pid)
                .unwrap_or(0);
            let core_rss = pid_rss_mib(core_pid);

            println!("=== GPUI + libghostty-vt + herdr-core Window Benchmark (106x32, 360 frames) ===");
            println!(
                "  first_frame_ms (incl. herdr-core + seq10k) : {:.1} ms",
                bench.first_paint_ms.unwrap_or(0.0)
            );
            println!(
                "  herdr_core_seq10k_roundtrip                : {:.1} ms ({:.1} KiB)",
                bench.seq_roundtrip_ms,
                bench.seq_bytes as f64 / 1024.0
            );
            println!(
                "  scroll_360_frames                          : p50={:.2}ms p95={:.2}ms max={:.2}ms >16.67ms={}/360 >25ms={}/360",
                p50, p95, max_ms, over_16, over_25
            );
            println!(
                "  rss_mib                                    : gpui={:.2} MiB, herdr-core={:.2} MiB, total={:.2} MiB",
                main_rss,
                core_rss,
                main_rss + core_rss
            );

            if let Some(mut sess) = bench.herdr_session.take() {
                bench.herdr_rt.block_on(sess.stop());
            }
            cx.quit();
            return;
        }

        window.request_animation_frame();
        cx.notify();
    }

    fn run_font_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ts = window.text_system();
        let all_fonts = ts.all_font_names();
        println!("=== GPUI Font & Glyph Shaping Probe ===");
        println!("Total system font families visible to GPUI: {}", all_fonts.len());
        let check_families = [
            "SF Mono",
            "Menlo",
            "UDEV Gothic 35NF",
            "UDEV Gothic NF",
            "Symbols Nerd Font Mono",
            "Hiragino Sans",
            "Noto Sans JP",
            "Apple Color Emoji",
        ];
        for fam in check_families {
            let found = all_fonts.iter().any(|f| f.eq_ignore_ascii_case(fam));
            println!("  family {:<24}: {}", fam, if found { "FOUND" } else { "missing" });
        }

        let font_size = px(DEFAULT_FONT_SIZE);
        let probe_glyphs = [
            ("ASCII 'A'", "A"),
            ("CJK 'あ'", "あ"),
            ("Half-width 'ｱ'", "ｱ"),
            ("NFD 'か\\u{3099}'", "か\u{3099}"),
            ("Emoji '🦀'", "🦀"),
            ("ZWJ '👨‍👩‍👧‍👦'", "👨‍👩‍👧‍👦"),
            ("NF Powerline U+E0B0", "\u{E0B0}"),
            ("NF Git U+E0A0", "\u{E0A0}"),
            ("NF Devicon U+E718", "\u{E718}"),
            ("NF FA Home U+F015", "\u{F015}"),
            ("NF MD Supp U+F024B", "\u{F024B}"),
        ];

        for (preset, nf_enabled) in [
            (FontPreset::Default, true),
            (FontPreset::UdevGothic35Nf, true),
            (FontPreset::Menlo, true),
            (FontPreset::Menlo, false),
        ] {
            let font = build_terminal_font(preset, nf_enabled);
            let font_id = ts.resolve_font(&font);
            let cell_advance = ts
                .advance(font_id, font_size, 'M')
                .map(|s| f32::from(s.width))
                .unwrap_or(0.0);
            let ascent = f32::from(ts.ascent(font_id, font_size));
            let descent = f32::from(ts.descent(font_id, font_size));
            let row_height = (DEFAULT_FONT_SIZE * DEFAULT_LINE_HEIGHT).ceil();
            println!(
                "\n--- Preset: {} (nerdFontFallback={}) ---",
                preset.label(),
                nf_enabled
            );
            println!(
                "  resolved_primary={:?} cell_width={:.3}px row_height={:.1}px ascent={:.2} descent={:.2}",
                ts.get_font_for_id(font_id).map(|f| f.family),
                cell_advance,
                row_height,
                ascent,
                descent
            );

            for (name, sample) in probe_glyphs {
                let run = TextRun {
                    len: sample.len(),
                    font: font.clone(),
                    color: rgb(FG_PRIMARY).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = ts.shape_line(sample.into(), font_size, &[run], None);
                let run_summary: Vec<String> = shaped
                    .runs
                    .iter()
                    .map(|r| {
                        let fam = ts
                            .get_font_for_id(r.font_id)
                            .map(|f| f.family.to_string())
                            .unwrap_or_else(|| format!("FontId({:?})", r.font_id));
                        let glyph_ids: Vec<String> =
                            r.glyphs.iter().map(|g| format!("{:?}", g.id)).collect();
                        let emoji_flags: Vec<bool> = r.glyphs.iter().map(|g| g.is_emoji).collect();
                        format!("{fam} glyphs={glyph_ids:?} emoji={emoji_flags:?}")
                    })
                    .collect();
                println!(
                    "  {:<22} width={:6.2}px cells={} -> {}",
                    name,
                    f32::from(shaped.width),
                    string_cell_width(sample),
                    run_summary.join(" | ")
                );
            }

            let ascii40_cols = string_cell_width("0123456789012345678901234567890123456789");
            let cjk20_cols = string_cell_width("あいうえおかきくけこさしすせそたちつてと");
            let ascii40_x = ascii40_cols as f32 * cell_advance;
            let cjk20_x = cjk20_cols as f32 * cell_advance;
            println!(
                "  grid_alignment: ascii40_x={:.3}px cjk20_x={:.3}px delta={:.3}px",
                ascii40_x,
                cjk20_x,
                (ascii40_x - cjk20_x).abs()
            );
        }

        self.probe_done = true;
        cx.quit();
    }
}

fn pid_rss_mib(pid: u32) -> f64 {
    if pid == 0 {
        return 0.0;
    }
    let Ok(out) = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
    else {
        return 0.0;
    };
    let kib: f64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0);
    kib / 1024.0
}

impl Focusable for GpuiPocView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for GpuiPocView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let (text, adjusted) = self.ime.text_for_range_utf16(range_utf16);
        *actual_range = Some(adjusted);
        text
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.ime.selected_range_utf16(),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime.marked_range_utf16()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime.unmark_text();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime.replace_text_in_range(range_utf16, text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime
            .replace_and_mark_text_in_range(range_utf16, new_text, new_selected_range_utf16);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_ime_line.as_ref()?;
        let range = self.ime.range_from_utf16(&range_utf16);
        let start_x = line.x_for_index(range.start);
        let end_x = line.x_for_index(range.end);
        let row_h = px((self.font_size * DEFAULT_LINE_HEIGHT).ceil());
        Some(Bounds::from_corners(
            point(bounds.left() + start_x, bounds.top()),
            point(bounds.left() + end_x.max(start_x + px(2.0)), bounds.top() + row_h),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_ime_bounds?;
        let line = self.last_ime_line.as_ref()?;
        let local_x = (point.x - bounds.left()).max(px(0.0));
        let utf8_idx = line.closest_index_for_x(local_x);
        Some(self.ime.offset_to_utf16(utf8_idx))
    }
}

/// Renders a live `libghostty-vt` `ScreenSnapshot` directly on GPUI Metal quads + shaped text runs.
struct VtScreenElement {
    view: Entity<GpuiPocView>,
}

struct VtPrepaint {
    cell_width: Pixels,
    row_height: Pixels,
    bg_quads: Vec<(Point<Pixels>, Pixels, u32)>,
    shaped_rows: Vec<(Pixels, Vec<(Pixels, ShapedLine)>)>,
}

impl IntoElement for VtScreenElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for VtScreenElement {
    type RequestLayoutState = ();
    type PrepaintState = VtPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("vt-screen-bench".into()))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (font, font_size, snapshot) = {
            let v = self.view.read(cx);
            (
                v.current_font(),
                px(v.font_size),
                v.bench.as_ref().and_then(|b| b.snapshot.clone()),
            )
        };
        let ts = window.text_system();
        let font_id = ts.resolve_font(&font);
        let cell_width = ts
            .advance(font_id, font_size, 'M')
            .map(|s| s.width)
            .unwrap_or(px(8.4));
        let row_height = px((f32::from(font_size) * DEFAULT_LINE_HEIGHT).ceil());

        let mut bg_quads = Vec::new();
        let mut shaped_rows = Vec::new();

        if let Some(snap) = snapshot {
            for (r_idx, cells) in snap.lines.iter().enumerate() {
                let y = row_height * r_idx as f32;
                let mut row_items = Vec::new();
                // Batch contiguous same-style ASCII/narrow runs into single `shape_line` calls
                // for realistic GPU terminal throughput while keeping exact cell alignment.
                let mut idx = 0;
                while idx < cells.len() {
                    let cell = &cells[idx];
                    if let Some((r, g, b)) = cell.bg_rgb {
                        let rgb_u32 = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                        bg_quads.push((
                            point(cell_width * cell.col as f32, y),
                            cell_width * cell.width.max(1) as f32,
                            rgb_u32,
                        ));
                    }
                    if !cell.invisible && !cell.text.is_empty() {
                        let fg_u32 = cell
                            .fg_rgb
                            .map(|(r, g, b)| ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
                            .unwrap_or(FG_PRIMARY);
                        let color = if cell.faint {
                            rgba((fg_u32 << 8) | 0x80).into()
                        } else {
                            rgb(fg_u32).into()
                        };
                        let start_col = cell.col;
                        let mut text_buf = cell.text.clone();
                        let mut expected_col = cell.col + cell.width as u16;
                        if cell.width == 1 && cell.text.is_ascii() {
                            while idx + 1 < cells.len() {
                                let next = &cells[idx + 1];
                                if next.col == expected_col
                                    && next.width == 1
                                    && next.text.is_ascii()
                                    && !next.invisible
                                    && next.fg_rgb == cell.fg_rgb
                                    && next.faint == cell.faint
                                    && next.bg_rgb.is_none()
                                {
                                    text_buf.push_str(&next.text);
                                    expected_col += 1;
                                    idx += 1;
                                } else {
                                    break;
                                }
                            }
                        }
                        let run = TextRun {
                            len: text_buf.len(),
                            font: font.clone(),
                            color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = ts.shape_line(text_buf.into(), font_size, &[run], None);
                        row_items.push((cell_width * start_col as f32, shaped));
                    }
                    idx += 1;
                }
                shaped_rows.push((y, row_items));
            }
        }

        VtPrepaint {
            cell_width,
            row_height,
            bg_quads,
            shaped_rows,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = point(bounds.left() + px(8.0), bounds.top() + px(6.0));
        let _ = prepaint.cell_width;
        for (pos, w, rgb_u32) in &prepaint.bg_quads {
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x + pos.x, origin.y + pos.y),
                    size(*w, prepaint.row_height),
                ),
                rgb(*rgb_u32),
            ));
        }
        for (y, row_items) in &prepaint.shaped_rows {
            for (x, shaped) in row_items {
                let _ = shaped.paint(
                    point(origin.x + *x, origin.y + *y),
                    prepaint.row_height,
                    window,
                    cx,
                );
            }
        }
    }
}

/// Custom GPUI Element that paints the cell-aligned terminal grid directly via `TextSystem`
/// and registers the `ElementInputHandler` for native macOS IME / macSKK support.
struct TerminalGridElement {
    view: Entity<GpuiPocView>,
}

struct GridPrepaintState {
    cell_width: Pixels,
    row_height: Pixels,
    grid_rows: Vec<(String, Vec<(Pixels, ShapedLine)>)>,
    faint_row_cells: Vec<(Pixels, bool, ShapedLine)>,
    ime_line: ShapedLine,
    ime_cursor_x: Pixels,
    ime_mark_span: Option<(Pixels, Pixels)>,
    ime_origin_y_offset: Pixels,
}

impl IntoElement for TerminalGridElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalGridElement {
    type RequestLayoutState = ();
    type PrepaintState = GridPrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("terminal-grid-poc".into()))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.0).into();
        style.size.height = gpui::relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (font, font_size, ime_content, ime_selected, ime_marked) = {
            let v = self.view.read(cx);
            (
                v.current_font(),
                px(v.font_size),
                v.ime.content.clone(),
                v.ime.selected_range.clone(),
                v.ime.marked_range.clone(),
            )
        };
        let ts = window.text_system();
        let font_id = ts.resolve_font(&font);
        let cell_width = ts
            .advance(font_id, font_size, 'M')
            .map(|s| s.width)
            .unwrap_or(px(8.5));
        let row_height = px((f32::from(font_size) * DEFAULT_LINE_HEIGHT).ceil());

        let mut grid_rows = Vec::with_capacity(PROBE_CASES.len());
        for case in PROBE_CASES {
            let mut shaped_cells = Vec::new();
            for cell in layout_cells(case.sample) {
                let run = TextRun {
                    len: cell.grapheme.len(),
                    font: font.clone(),
                    color: rgb(FG_PRIMARY).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = ts.shape_line(cell.grapheme.to_string().into(), font_size, &[run], None);
                shaped_cells.push((cell_width * cell.col as f32, shaped));
            }
            grid_rows.push((case.label.to_string(), shaped_cells));
        }

        let faint_segments = [
            ("Normal ", false, false),
            ("DIM ", true, false),
            ("normal ", false, false),
            ("hidden", false, true),
        ];
        let mut faint_row_cells = Vec::new();
        let mut col_cursor = 0usize;
        for (seg_text, is_dim, is_hidden) in faint_segments {
            for cell in layout_cells(seg_text) {
                let fg_color = if is_hidden {
                    rgba(0x00000000).into()
                } else if is_dim {
                    rgba((FG_PRIMARY << 8) | 0x80).into()
                } else {
                    rgb(FG_PRIMARY).into()
                };
                let run = TextRun {
                    len: cell.grapheme.len(),
                    font: font.clone(),
                    color: fg_color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = ts.shape_line(cell.grapheme.to_string().into(), font_size, &[run], None);
                faint_row_cells.push((cell_width * col_cursor as f32, is_hidden, shaped));
                col_cursor += cell.width;
            }
        }

        let display_text: SharedString = if ime_content.is_empty() {
            "ここに日本語(macSKK/標準IME)を入力してEnter/Esc/Cmd+Tabを検証...".into()
        } else {
            ime_content.clone().into()
        };
        let ime_runs = if ime_content.is_empty() {
            vec![TextRun {
                len: display_text.len(),
                font: font.clone(),
                color: rgb(FG_MUTED).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }]
        } else if let Some(mark) = &ime_marked {
            let mut runs = Vec::new();
            if mark.start > 0 {
                runs.push(TextRun {
                    len: mark.start,
                    font: font.clone(),
                    color: rgb(FG_PRIMARY).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
            if !mark.is_empty() {
                runs.push(TextRun {
                    len: mark.end - mark.start,
                    font: font.clone(),
                    color: rgb(ACCENT_YELLOW).into(),
                    background_color: None,
                    underline: Some(UnderlineStyle {
                        thickness: px(1.5),
                        color: Some(rgb(ACCENT_YELLOW).into()),
                        wavy: false,
                    }),
                    strikethrough: None,
                });
            }
            if mark.end < ime_content.len() {
                runs.push(TextRun {
                    len: ime_content.len() - mark.end,
                    font: font.clone(),
                    color: rgb(FG_PRIMARY).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                });
            }
            runs
        } else {
            vec![TextRun {
                len: ime_content.len(),
                font: font.clone(),
                color: rgb(FG_PRIMARY).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }]
        };
        let ime_line = ts.shape_line(display_text, font_size, &ime_runs, None);
        let ime_cursor_x = if ime_content.is_empty() {
            px(0.0)
        } else {
            ime_line.x_for_index(ime_selected.end)
        };
        let ime_mark_span = ime_marked.as_ref().and_then(|mark| {
            if mark.is_empty() {
                None
            } else {
                Some((ime_line.x_for_index(mark.start), ime_line.x_for_index(mark.end)))
            }
        });
        let ime_origin_y_offset = row_height * (PROBE_CASES.len() as f32 + 3.0);

        GridPrepaintState {
            cell_width,
            row_height,
            grid_rows,
            faint_row_cells,
            ime_line,
            ime_cursor_x,
            ime_mark_span,
            ime_origin_y_offset,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let ime_box_bounds = Bounds::new(
            point(bounds.left() + px(16.0), bounds.top() + prepaint.ime_origin_y_offset),
            size(
                (bounds.size.width - px(32.0)).max(px(120.0)),
                prepaint.row_height + px(12.0),
            ),
        );
        let ime_text_origin = point(ime_box_bounds.left() + px(8.0), ime_box_bounds.top() + px(6.0));
        let ime_text_bounds = Bounds::new(
            ime_text_origin,
            size(ime_box_bounds.size.width - px(16.0), prepaint.row_height),
        );

        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(ime_text_bounds, self.view.clone()),
            cx,
        );

        let label_col_w = px(150.0);
        let grid_left = bounds.left() + px(16.0) + label_col_w;
        let mut y = bounds.top() + px(10.0);

        for (label, cells) in &prepaint.grid_rows {
            let label_font = self.view.read(cx).current_font();
            let label_run = TextRun {
                len: label.len(),
                font: label_font,
                color: rgb(ACCENT_CYAN).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped_label = window.text_system().shape_line(
                label.clone().into(),
                px(12.0),
                &[label_run],
                None,
            );
            let _ = shaped_label.paint(
                point(bounds.left() + px(16.0), y),
                prepaint.row_height,
                window,
                cx,
            );

            for (x_offset, shaped_cell) in cells {
                let _ = shaped_cell.paint(
                    point(grid_left + *x_offset, y),
                    prepaint.row_height,
                    window,
                    cx,
                );
            }
            y += prepaint.row_height;
        }

        y += prepaint.row_height * 0.4;
        let faint_bg_bounds = Bounds::new(
            point(grid_left, y),
            size(prepaint.cell_width * 41.0, prepaint.row_height),
        );
        window.paint_quad(fill(faint_bg_bounds, rgb(FAINT_BG)));
        for (x_offset, is_hidden, shaped) in &prepaint.faint_row_cells {
            if !*is_hidden {
                let _ = shaped.paint(
                    point(grid_left + *x_offset, y),
                    prepaint.row_height,
                    window,
                    cx,
                );
            }
        }

        window.paint_quad(PaintQuad {
            bounds: ime_box_bounds,
            corner_radii: px(4.0).into(),
            background: rgb(BG_SURFACE).into(),
            border_widths: px(1.0).into(),
            border_color: if focus_handle.is_focused(window) {
                rgb(ACCENT_BLUE).into()
            } else {
                rgb(BORDER_SUBTLE).into()
            },
            border_style: Default::default(),
        });

        if let Some((m_start, m_end)) = prepaint.ime_mark_span {
            let mark_bg = Bounds::new(
                point(ime_text_origin.x + m_start, ime_text_origin.y),
                size((m_end - m_start).max(px(2.0)), prepaint.row_height),
            );
            window.paint_quad(fill(mark_bg, hsla(0.13, 0.85, 0.5, 0.18)));
        }

        let _ = prepaint
            .ime_line
            .paint(ime_text_origin, prepaint.row_height, window, cx);

        let cursor_quad = fill(
            Bounds::new(
                point(ime_text_origin.x + prepaint.ime_cursor_x, ime_text_origin.y),
                size(px(2.0), prepaint.row_height),
            ),
            rgb(ACCENT_GREEN),
        );
        window.paint_quad(cursor_quad);

        let line_clone = prepaint.ime_line.clone();
        self.view.update(cx, |view, _| {
            view.last_ime_bounds = Some(ime_text_bounds);
            view.last_ime_line = Some(line_clone);
        });
    }
}

impl Render for GpuiPocView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.mode == RunMode::ProbeFonts && !self.probe_done {
            self.run_font_probe(window, cx);
        } else if self.mode == RunMode::BenchWindow {
            self.step_bench_frame(window, cx);
            return div()
                .size_full()
                .bg(rgb(BG_PRIMARY))
                .child(VtScreenElement {
                    view: cx.entity().clone(),
                });
        }

        let font = self.current_font();
        let font_id = window.text_system().resolve_font(&font);
        let cell_w = window
            .text_system()
            .advance(font_id, px(self.font_size), 'M')
            .map(|s| f32::from(s.width))
            .unwrap_or(0.0);
        let row_h = (self.font_size * DEFAULT_LINE_HEIGHT).ceil();

        let ime_status = format!(
            "active={} composing={} utf16_len={} marked={:?} committed_lines={} events={}",
            self.window_active,
            self.ime.is_composing(),
            self.ime.content.encode_utf16().count(),
            self.ime.marked_range_utf16(),
            self.ime.committed_lines.len(),
            self.ime.recent_events.len()
        );

        div()
            .key_context("GpuiPoc")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_backspace))
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_zoom_reset))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.focus(&this.focus_handle);
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG_PRIMARY))
            .text_color(rgb(FG_PRIMARY))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(rgb(BG_SURFACE))
                    .border_b_1()
                    .border_color(rgb(BORDER_SUBTLE))
                    .text_size(px(12.0))
                    .children(FontPreset::ALL.into_iter().map(|preset| {
                        let selected = self.preset == preset;
                        div()
                            .id(SharedString::from(format!("preset-{}", preset.label())))
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(if selected { ACCENT_BLUE } else { BORDER_SUBTLE }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.preset = preset;
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }),
                            )
                            .child(preset.label())
                    }))
                    .child(
                        div()
                            .id("toggle-nf")
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(if self.nerd_font_fallback {
                                ACCENT_GREEN
                            } else {
                                BORDER_SUBTLE
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.nerd_font_fallback = !this.nerd_font_fallback;
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }),
                            )
                            .child(if self.nerd_font_fallback {
                                "NF Fallback: ON"
                            } else {
                                "NF Fallback: OFF"
                            }),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_color(rgb(FG_MUTED))
                            .child(format!(
                                "{:.1}px | cell {:.2}x{:.0}px",
                                self.font_size, cell_w, row_h
                            )),
                    ),
            )
            .child(div().flex_1().w_full().child(TerminalGridElement {
                view: cx.entity().clone(),
            }))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .bg(rgb(BG_SURFACE))
                    .border_t_1()
                    .border_color(rgb(BORDER_SUBTLE))
                    .text_size(px(11.0))
                    .text_color(rgb(FG_MUTED))
                    .child(format!("IME Diagnostics (privacy-safe): {ime_status}")),
            )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--bench-vt") {
        vt::run_headless_vt_benchmark();
        return;
    }

    let mode = if args.iter().any(|a| a == "--probe-fonts") {
        RunMode::ProbeFonts
    } else if args.iter().any(|a| a == "--bench-window") {
        RunMode::BenchWindow
    } else {
        RunMode::Interactive
    };

    let started_at = Instant::now();
    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", ImeBackspace, Some("GpuiPoc")),
            KeyBinding::new("enter", ImeEnter, Some("GpuiPoc")),
            KeyBinding::new("escape", ImeEscape, Some("GpuiPoc")),
            KeyBinding::new("cmd-=", FontZoomIn, Some("GpuiPoc")),
            KeyBinding::new("cmd--", FontZoomOut, Some("GpuiPoc")),
            KeyBinding::new("cmd-0", FontZoomReset, Some("GpuiPoc")),
            KeyBinding::new("cmd-q", QuitPoc, None),
        ]);
        cx.on_action(|_: &QuitPoc, cx| cx.quit());

        let win_size = if mode == RunMode::BenchWindow {
            size(px(939.0), px(600.0))
        } else {
            size(px(920.0), px(420.0))
        };
        let bounds = Bounds::centered(None, win_size, cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| GpuiPocView::new(mode, started_at, window, cx)),
        )
        .expect("open GPUI PoC window");
        cx.activate(true);
    });
}
