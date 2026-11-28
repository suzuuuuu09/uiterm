mod diagnostics;
mod fonts;

use diagnostics::GpuiPocView;
mod herdr;
mod ime;
mod input;
mod locale;
mod os;
mod renderer;
mod vt;

use std::ops::Range;
use std::time::{Duration, Instant};

use fonts::{
    DEFAULT_FONT_SIZE, DEFAULT_LINE_HEIGHT, FontPreset, MAX_FONT_SIZE, MIN_FONT_SIZE,
    build_terminal_font, layout_cells, string_cell_width,
};
use ghostty_vt::key::{Key as VtKey, Mods as VtMods};
use ghostty_vt::mouse::{
    Action as VtMouseAction, Button as VtMouseButton, EncoderSize as VtMouseSize,
};
use ghostty_vt::screen::Screen;
use ghostty_vt::terminal::ScrollViewport;
use gpui::{
    App, Application, Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, Font, GlobalElementId, HitboxBehavior,
    InputHandler, InspectorElementId, IntoElement, KeyBinding, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render,
    ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, WindowBounds, WindowOptions, actions, div, fill, hsla, point, prelude::*, px, rgb,
    rgba, size,
};
use herdr::{
    CloseInfo, DirCleanupGuard, HerdrGuard, HerdrHandle, HerdrOutputStream, HerdrStartOptions,
    SessionOutput,
};
use ime::{ImeBuffer, TerminalImeState};
use input::{KeyDispatchDecision, TerminalAppAction, classify_keydown};
use renderer::{
    ImeOverlayState, PreparedTerminalFrame, RenderCacheMode, RenderStyleConfig, RowShapeCache,
    measure_cell_metrics, paint_terminal_frame,
};
use vt::{
    DEFAULT_SCROLLBACK_LINES, MAX_GRID_DIMENSION, MAX_SCROLLBACK_LINES, VtTerminal,
    current_rss_mib, make_10k_payload,
};

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
const ACCENT_RED: u32 = 0xef4444;
const ACCENT_YELLOW: u32 = 0xeab308;
const ACCENT_CYAN: u32 = 0x06b6d4;
const FAINT_BG: u32 = 0x406090;

const TERM_PAD_X: f32 = 8.0;
const TERM_PAD_Y: f32 = 6.0;
const SYNC_HOLD_TIMEOUT: Duration = Duration::from_millis(100);
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

fn install_signal_cleanup_watcher(
    runtime: &tokio::runtime::Runtime,
) -> std::io::Result<tokio::task::JoinHandle<libc::c_int>> {
    use tokio::signal::unix::{SignalKind, signal};

    let _entered = runtime.enter();
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(runtime.spawn(async move {
        tokio::select! {
            _ = terminate.recv() => libc::SIGTERM,
            _ = interrupt.recv() => libc::SIGINT,
        }
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    LiveTerminal,
    ProbeUi,
    ProbeFonts,
    BenchWindow,
    BenchVt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchScenario {
    ScrollStream,
    SingleRow,
    ScrollHistory1,
    ScrollHistory6,
    NvimCtrlDu,
    Burst5MiB,
    InputLatency,
}

impl BenchScenario {
    fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "single-row" => Self::SingleRow,
            "scroll-history" | "scroll-history-1" => Self::ScrollHistory1,
            "scroll-history-6" => Self::ScrollHistory6,
            "nvim-ctrl-du" => Self::NvimCtrlDu,
            "burst-5mib" => Self::Burst5MiB,
            "input-latency" => Self::InputLatency,
            _ => Self::ScrollStream,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ScrollStream => "scroll-stream",
            Self::SingleRow => "single-row",
            Self::ScrollHistory1 => "scroll-history-1",
            Self::ScrollHistory6 => "scroll-history-6",
            Self::NvimCtrlDu => "nvim-ctrl-du",
            Self::Burst5MiB => "burst-5mib",
            Self::InputLatency => "input-latency",
        }
    }
}

#[derive(Debug, Clone)]
struct CliConfig {
    mode: RunMode,
    cols: u16,
    rows: u16,
    font_preset: FontPreset,
    font_size: f32,
    scrollback_lines: usize,
    render_mode: RenderCacheMode,
    minimal: bool,
    wrap_iutf8: bool,
    cmd_argv: Option<Vec<String>>,
    scenario: BenchScenario,
}

impl CliConfig {
    fn parse(args: &[String]) -> Self {
        let mut mode = RunMode::LiveTerminal;
        let mut cols = 106u16;
        let mut rows = 32u16;
        let mut font_preset = FontPreset::Default;
        let mut font_size = DEFAULT_FONT_SIZE;
        let mut scrollback_lines = DEFAULT_SCROLLBACK_LINES;
        let mut render_mode = RenderCacheMode::DirtyRowCache;
        let mut minimal = false;
        let mut wrap_iutf8 = true;
        let mut cmd_argv: Option<Vec<String>> = None;
        let mut scenario = BenchScenario::ScrollStream;

        let mut i = 1usize;
        while i < args.len() {
            match args[i].as_str() {
                "--probe-fonts" => mode = RunMode::ProbeFonts,
                "--probe-ui" => mode = RunMode::ProbeUi,
                "--bench-window" => mode = RunMode::BenchWindow,
                "--bench-vt" => mode = RunMode::BenchVt,
                "--minimal" => minimal = true,
                "--no-iutf8-wrap" => wrap_iutf8 = false,
                "--cols" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<u16>().ok()) {
                        cols = v.clamp(1, MAX_GRID_DIMENSION);
                        i += 1;
                    }
                }
                "--rows" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<u16>().ok()) {
                        rows = v.clamp(1, MAX_GRID_DIMENSION);
                        i += 1;
                    }
                }
                "--font-size" => {
                    if let Some(v) = args
                        .get(i + 1)
                        .and_then(|s| s.parse::<f32>().ok())
                        .filter(|size| size.is_finite())
                    {
                        font_size = v.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                        i += 1;
                    }
                }
                "--font-preset" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| FontPreset::from_cli_name(s)) {
                        font_preset = v;
                        i += 1;
                    }
                }
                "--scrollback-lines" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                        scrollback_lines = v.min(MAX_SCROLLBACK_LINES);
                        i += 1;
                    }
                }
                "--render-mode" => {
                    if let Some(v) = args.get(i + 1) {
                        render_mode = if v == "uncached" {
                            RenderCacheMode::Uncached
                        } else {
                            RenderCacheMode::DirtyRowCache
                        };
                        i += 1;
                    }
                }
                "--scenario" => {
                    if let Some(v) = args.get(i + 1) {
                        scenario = BenchScenario::from_str(v);
                        i += 1;
                    }
                }
                "--shell" => {
                    if let Some(v) = args.get(i + 1) {
                        let argv = if os::supports_login_flag(v) {
                            vec![v.clone(), "-l".to_string()]
                        } else {
                            vec![v.clone()]
                        };
                        cmd_argv = Some(argv);
                        i += 1;
                    }
                }
                "--cmd" if i + 1 < args.len() => {
                    cmd_argv = Some(args[i + 1..].to_vec());
                    break;
                }
                _ => {}
            }
            i += 1;
        }

        Self {
            mode,
            cols,
            rows,
            font_preset,
            font_size,
            scrollback_lines,
            render_mode,
            minimal,
            wrap_iutf8,
            cmd_argv,
            scenario,
        }
    }
}

// ---------------------------------------------------------------------------
// Live Interactive Terminal View
// ---------------------------------------------------------------------------

struct LiveTerminalView {
    config: CliConfig,
    focus_handle: FocusHandle,
    style: RenderStyleConfig,
    vt: VtTerminal,
    cache: RowShapeCache,
    presented_frame: Option<std::rc::Rc<PreparedTerminalFrame>>,
    ime: TerminalImeState,
    herdr_guard: Option<HerdrGuard>,
    herdr_handle: Option<HerdrHandle>,
    session_closed: Option<CloseInfo>,
    locale: locale::Locale,
    session_error: Option<String>,
    window_active: bool,
    cursor_blink_on: bool,
    cursor_blink_armed: bool,
    sync_timer_armed: bool,
    pending_title: Option<String>,
    last_content_bounds: Option<Bounds<Pixels>>,
    last_requested_grid: Option<(u16, u16)>,
    mouse_presses: Vec<(MouseButton, bool)>,
    last_cell_w: f32,
    last_row_h: f32,
    last_ime_anchor: Option<Bounds<Pixels>>,
    last_ime_overlay: Option<(Bounds<Pixels>, ShapedLine)>,
    scroll_px_accum: f32,
    startup_task: Option<gpui::Task<()>>,
    _output_task: Option<gpui::Task<()>>,
}

impl LiveTerminalView {
    fn new(config: CliConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        let style = RenderStyleConfig {
            preset: config.font_preset,
            nerd_font_fallback: true,
            font_size: config.font_size,
            line_height: DEFAULT_LINE_HEIGHT,
        };
        let vt = VtTerminal::new(config.cols, config.rows, config.scrollback_lines)
            .expect("create VtTerminal");
        let cache = RowShapeCache::new(config.render_mode);

        let mut view = Self {
            config: config.clone(),
            focus_handle,
            style,
            vt,
            cache,
            presented_frame: None,
            ime: TerminalImeState::new(),
            herdr_guard: None,
            herdr_handle: None,
            session_closed: None,
            locale: locale::Locale::system(),
            session_error: None,
            window_active: window.is_window_active(),
            cursor_blink_on: true,
            cursor_blink_armed: false,
            sync_timer_armed: false,
            pending_title: Some("uiterm".to_string()),
            last_content_bounds: None,
            last_requested_grid: None,
            mouse_presses: Vec::new(),
            last_cell_w: 8.43,
            last_row_h: (config.font_size * DEFAULT_LINE_HEIGHT).ceil(),
            last_ime_anchor: None,
            last_ime_overlay: None,
            scroll_px_accum: 0.0,
            startup_task: None,
            _output_task: None,
        };

        view.start_session(cx);

        cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            if this.window_active != active {
                this.window_active = active;
                if active {
                    this.ime.on_window_focus();
                    this.cursor_blink_on = true;
                } else {
                    this.ime.on_window_blur();
                }
                if let Some(handle) = &this.herdr_handle
                    && let Err(error) = handle.try_send_focus(active)
                {
                    this.session_error = Some(error);
                }
                cx.notify();
            }
        })
        .detach();

        let view_weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_window, cx| {
            let _ = view_weak.update(cx, |this, _| {
                this.shutdown();
            });
            true
        });

        cx.on_app_quit(|this, _cx| {
            this.shutdown();
            std::future::ready(())
        })
        .detach();

        view
    }

    fn start_session(&mut self, cx: &mut Context<Self>) {
        self.mouse_presses.clear();
        self.shutdown();
        self.session_closed = None;
        self.session_error = None;
        self.vt = VtTerminal::new(self.vt.cols(), self.vt.rows(), self.config.scrollback_lines)
            .expect("recreate VtTerminal");
        self.cache.clear();
        self.presented_frame = None;
        self.last_requested_grid = None;

        let argv = self
            .config
            .cmd_argv
            .clone()
            .unwrap_or_else(os::default_shell_argv);
        let shell = argv.first().map(String::as_str);
        let env = os::build_child_shell_env(shell);
        let opts = HerdrStartOptions {
            argv: Some(argv),
            cwd: None,
            env,
            cols: self.vt.cols(),
            rows: self.vt.rows(),
            history_lines: self.config.scrollback_lines,
            wrap_iutf8: self.config.wrap_iutf8,
        };

        let startup = cx
            .background_executor()
            .spawn(async move { HerdrGuard::start_live(opts) });
        self.startup_task = Some(cx.spawn(async move |this, cx| {
            let result = startup.await;
            let _ = this.update(cx, |view, cx| {
                view.startup_task = None;
                match result {
                    Ok((guard, handle, stream)) => {
                        view.herdr_guard = Some(guard);
                        view.start_output_pump(handle, stream, cx);
                    }
                    Err(error) => {
                        let mut message = error.to_string();
                        if error.code == "herdr_core_not_found" || error.code == "invalid_core_path" {
                            message.push(' ');
                            message.push_str(view.locale.label(
                                "Run inside nix develop or set HERDR_CORE_BIN to an absolute path.",
                                "nix develop で起動するか、HERDR_CORE_BIN に絶対パスを指定してください。",
                            ));
                        }
                        view.session_error = Some(message);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn start_output_pump(
        &mut self,
        handle: HerdrHandle,
        mut stream: HerdrOutputStream,
        cx: &mut Context<Self>,
    ) {
        self.session_error = handle.try_send_focus(self.window_active).err();
        self.herdr_handle = Some(handle);
        self._output_task = Some(cx.spawn(async move |this, cx| {
            Self::pump_pty_output(this, &mut stream, cx).await;
        }));
    }

    fn reattach(&mut self, cx: &mut Context<Self>) {
        let Some(guard) = &mut self.herdr_guard else {
            return;
        };
        self._output_task = None;
        self.herdr_handle = None;
        let (cols, rows) = self
            .last_requested_grid
            .unwrap_or((self.vt.cols(), self.vt.rows()));
        let opts = HerdrStartOptions {
            cols,
            rows,
            history_lines: self.config.scrollback_lines,
            ..HerdrStartOptions::default()
        };
        match guard.reattach(opts) {
            Ok((handle, stream)) => {
                self.mouse_presses.clear();
                self.vt = VtTerminal::new(cols, rows, self.config.scrollback_lines)
                    .expect("reset VT before replay");
                self.cache.clear();
                self.presented_frame = None;
                self.last_requested_grid = None;
                self.session_closed = None;
                self.start_output_pump(handle, stream, cx);
            }
            Err(error) => self.session_error = Some(error),
        }
    }

    async fn pump_pty_output(
        this: gpui::WeakEntity<Self>,
        stream: &mut HerdrOutputStream,
        cx: &mut gpui::AsyncApp,
    ) {
        const MAX_BATCH_BYTES: usize = 256 * 1024;
        let mut pending = None;
        loop {
            let first = match pending.take() {
                Some(event) => event,
                None => match stream.recv_uncredited().await {
                    Some(event) => event,
                    None => break,
                },
            };

            let mut data_buf = Vec::new();
            let mut resize = None;
            let mut closed = None;
            match first {
                SessionOutput::Data(bytes) => {
                    data_buf = bytes;
                    while data_buf.len() < MAX_BATCH_BYTES {
                        match stream.try_recv_uncredited() {
                            Some(SessionOutput::Data(bytes)) => data_buf.extend_from_slice(&bytes),
                            event => {
                                pending = event;
                                break;
                            }
                        }
                    }
                }
                SessionOutput::Resize { cols, rows } => resize = Some((cols, rows)),
                SessionOutput::Closed(info) => closed = Some(info),
            }

            let Ok(replies) = this.update(cx, |view, cx| {
                let mut replies = Vec::new();
                if !data_buf.is_empty() {
                    let events = view.vt.feed(&data_buf);
                    replies = events.pty_writes;
                    if let Some(title) = events.title_changed
                        && !title.is_empty()
                    {
                        view.pending_title = Some(title);
                    }
                    if view.vt.is_sync_held() {
                        view.arm_sync_hold_timer(cx);
                    } else {
                        cx.notify();
                    }
                }
                if let Some((cols, rows)) = resize
                    && (cols, rows) != (view.vt.cols(), view.vt.rows())
                {
                    let cell_w = (view.last_cell_w.round() as u32).max(1);
                    let row_h = (view.last_row_h.round() as u32).max(1);
                    match view.vt.resize(cols, rows, cell_w, row_h) {
                        Ok(reports) => {
                            replies = reports;
                            view.cache.clear();
                        }
                        Err(error) => view.session_error = Some(error),
                    }
                    cx.notify();
                }
                if let Some(info) = closed {
                    view.herdr_handle = None;
                    view.session_error = None;
                    view.session_closed = Some(info);
                    cx.notify();
                }
                replies
            }) else {
                break;
            };
            if (!data_buf.is_empty() || !replies.is_empty())
                && let Err(error) = stream.acknowledge(data_buf.len() as u32, replies).await
            {
                let _ = this.update(cx, |view, cx| {
                    view.session_error = Some(error);
                    cx.notify();
                });
            }
            if stream_closed(&this, cx) {
                break;
            }
        }
    }

    fn arm_sync_hold_timer(&mut self, cx: &mut Context<Self>) {
        if self.sync_timer_armed {
            return;
        }
        let Some(remaining) = self.vt.sync_hold_remaining(SYNC_HOLD_TIMEOUT) else {
            return;
        };
        self.sync_timer_armed = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(remaining).await;
            let _ = this.update(cx, |view, cx| {
                view.sync_timer_armed = false;
                if view.vt.release_expired_sync_hold(SYNC_HOLD_TIMEOUT) {
                    cx.notify();
                } else {
                    view.arm_sync_hold_timer(cx);
                }
            });
        })
        .detach();
    }

    fn maybe_arm_cursor_blink(&mut self, should_blink: bool, cx: &mut Context<Self>) {
        if !should_blink || !self.window_active || self.session_closed.is_some() {
            return;
        }
        if self.cursor_blink_armed {
            return;
        }
        self.cursor_blink_armed = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
            let _ = this.update(cx, |view, cx| {
                view.cursor_blink_armed = false;
                if view.window_active && view.session_closed.is_none() {
                    view.cursor_blink_on = !view.cursor_blink_on;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn shutdown(&mut self) {
        self.startup_task = None;
        self._output_task = None;
        self.herdr_handle = None;
        if let Some(mut guard) = self.herdr_guard.take() {
            guard.stop_blocking();
        }
    }

    fn send_user_bytes(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.cursor_blink_on = true;
        if let Some(handle) = &self.herdr_handle {
            self.session_error = handle.try_send_user(bytes).err();
        }
    }

    fn perform_app_action(
        &mut self,
        action: TerminalAppAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TerminalAppAction::CopySelection => {
                if let Some(text) = self.vt.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            TerminalAppAction::Paste => {
                if let Some(item) = cx.read_from_clipboard()
                    && let Some(text) = item.text()
                {
                    self.vt.clear_selection();
                    let bytes = self.vt.encode_paste(&text);
                    self.send_user_bytes(bytes);
                    cx.notify();
                }
            }
            TerminalAppAction::SelectAll => {
                self.vt.select_all();
                cx.notify();
            }
            TerminalAppAction::ZoomIn => {
                self.style.font_size = (self.style.font_size + 1.0).min(MAX_FONT_SIZE);
                self.cache.clear();
                cx.notify();
            }
            TerminalAppAction::ZoomOut => {
                self.style.font_size = (self.style.font_size - 1.0).max(MIN_FONT_SIZE);
                self.cache.clear();
                cx.notify();
            }
            TerminalAppAction::ZoomReset => {
                self.style.font_size = DEFAULT_FONT_SIZE;
                self.cache.clear();
                cx.notify();
            }
            TerminalAppAction::CycleFont => {
                self.style.preset = self.style.preset.next();
                self.cache.clear();
                cx.notify();
            }
            TerminalAppAction::ToggleNerdFallback => {
                self.style.nerd_font_fallback = !self.style.nerd_font_fallback;
                self.cache.clear();
                cx.notify();
            }
            TerminalAppAction::RestartSession => {
                self.start_session(cx);
                window.focus(&self.focus_handle);
                cx.notify();
            }
            #[cfg(target_os = "macos")]
            TerminalAppAction::ToggleRenderCache => {
                self.cache.mode = match self.cache.mode {
                    RenderCacheMode::DirtyRowCache => RenderCacheMode::Uncached,
                    RenderCacheMode::Uncached => RenderCacheMode::DirtyRowCache,
                };
                self.cache.clear();
                cx.notify();
            }
            #[cfg(target_os = "macos")]
            TerminalAppAction::Quit => {
                self.shutdown();
                cx.quit();
            }
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.startup_task.is_some() {
            cx.stop_propagation();
            return;
        }
        let is_composing = self.ime.is_composing();
        let has_selection = self.vt.has_selection();
        match classify_keydown(
            &event.keystroke,
            is_composing,
            has_selection,
            self.vt.mac_option_as_alt,
        ) {
            KeyDispatchDecision::PassToIme => {
                // Propagate to NSTextInputContext -> EntityInputHandler
            }
            KeyDispatchDecision::ConsumeNoPty => {
                cx.stop_propagation();
            }
            KeyDispatchDecision::AppAction(action) => {
                self.perform_app_action(action, window, cx);
                cx.stop_propagation();
            }
            KeyDispatchDecision::SendToTerminal {
                key,
                mods,
                utf8,
                unshifted_codepoint,
            } => {
                if has_selection {
                    self.vt.clear_selection();
                }
                let seq = self
                    .vt
                    .encode_key(key, mods, utf8.as_deref(), unshifted_codepoint);
                self.send_user_bytes(seq);
                cx.stop_propagation();
                cx.notify();
            }
        }
    }

    fn pointer_to_grid(&self, pos: Point<Pixels>) -> (u16, u16, f64, f64) {
        let origin = self
            .last_content_bounds
            .map(|b| b.origin)
            .unwrap_or_else(|| point(px(0.0), px(0.0)));
        let local_x = (f32::from(pos.x - origin.x)).max(0.0);
        let local_y = (f32::from(pos.y - origin.y)).max(0.0);
        let cw = self.last_cell_w.max(1.0);
        let rh = self.last_row_h.max(1.0);
        let (cols, rows) = self.presented_grid();
        let col = ((local_x / cw).floor() as u16).min(cols.saturating_sub(1));
        let row = ((local_y / rh).floor() as u16).min(rows.saturating_sub(1));
        (col, row, local_x as f64, local_y as f64)
    }

    fn presented_grid(&self) -> (u16, u16) {
        self.presented_frame
            .as_ref()
            .map_or((self.vt.cols(), self.vt.rows()), |frame| {
                (frame.meta.cols, frame.meta.rows)
            })
    }

    fn mouse_encoder_position(&self, x: f64, y: f64) -> (f32, f32) {
        self.vt.mouse_encoder_position(
            x as f32,
            y as f32,
            self.last_cell_w,
            self.last_row_h,
            self.mouse_encoder_size(),
        )
    }

    fn mouse_encoder_size(&self) -> VtMouseSize {
        let (w, h) = self
            .last_content_bounds
            .map(|b| {
                (
                    f32::from(b.size.width).round() as u32,
                    f32::from(b.size.height).round() as u32,
                )
            })
            .unwrap_or((800, 500));
        let cell_width = (self.last_cell_w.round() as u32).max(1);
        let cell_height = (self.last_row_h.round() as u32).max(1);
        let (screen_width, screen_height) = if self.vt.uses_pixel_mouse_coordinates() {
            (w.max(1), h.max(1))
        } else {
            let (cols, rows) = self.presented_grid();
            (u32::from(cols) * cell_width, u32::from(rows) * cell_height)
        };
        VtMouseSize {
            screen_width,
            screen_height,
            cell_width,
            cell_height,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        let (col, row, lx, ly) = self.pointer_to_grid(event.position);

        let to_pty = self.vt.is_mouse_tracking() && !event.modifiers.shift;
        self.mouse_presses
            .retain(|(button, _)| *button != event.button);
        if to_pty {
            let (x, y) = self.mouse_encoder_position(lx, ly);
            if let Some(btn) = gpui_button_to_vt(event.button) {
                let bytes = self.vt.encode_mouse(
                    VtMouseAction::Press,
                    Some(btn),
                    input::gpui_mods_to_ghostty(&event.modifiers),
                    x,
                    y,
                    true,
                    self.mouse_encoder_size(),
                );
                if !bytes.is_empty() && self.herdr_handle.is_some() {
                    self.send_user_bytes(bytes);
                    if self.session_error.is_none() {
                        self.mouse_presses.push((event.button, true));
                    }
                }
            }
            return;
        }

        if event.button == MouseButton::Left && !self.vt.is_sync_held() {
            self.mouse_presses.push((event.button, false));
            self.vt.selection_press(col, row, lx, ly, self.last_cell_w);
            cx.notify();
        }
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (col, row, lx, ly) = self.pointer_to_grid(event.position);
        let to_pty = should_route_mouse_move_to_pty(
            event.pressed_button,
            &self.mouse_presses,
            self.vt.is_mouse_tracking(),
            event.modifiers.shift,
            self.last_content_bounds,
            event.position,
        );
        if to_pty {
            let (x, y) = self.mouse_encoder_position(lx, ly);
            let btn = event.pressed_button.and_then(gpui_button_to_vt);
            let bytes = self.vt.encode_mouse(
                VtMouseAction::Motion,
                btn,
                input::gpui_mods_to_ghostty(&event.modifiers),
                x,
                y,
                btn.is_some(),
                self.mouse_encoder_size(),
            );
            self.send_user_bytes(bytes);
            return;
        }

        if event.pressed_button == Some(MouseButton::Left)
            && self.mouse_presses.contains(&(MouseButton::Left, false))
            && !self.vt.is_sync_held()
        {
            let bounds = self.last_content_bounds.unwrap_or_default();
            self.vt.selection_drag(
                col,
                row,
                (lx, ly),
                event.modifiers.alt,
                self.last_cell_w,
                (f32::from(bounds.size.height).round() as u32).max(1),
            );
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let (col, row, lx, ly) = self.pointer_to_grid(event.position);
        let Some(index) = self
            .mouse_presses
            .iter()
            .position(|(button, _)| *button == event.button)
        else {
            return;
        };
        let (_, to_pty) = self.mouse_presses.swap_remove(index);
        if to_pty {
            let (x, y) = self.mouse_encoder_position(lx, ly);
            if let Some(btn) = gpui_button_to_vt(event.button) {
                let bytes = self.vt.encode_mouse(
                    VtMouseAction::Release,
                    Some(btn),
                    input::gpui_mods_to_ghostty(&event.modifiers),
                    x,
                    y,
                    false,
                    self.mouse_encoder_size(),
                );
                self.send_user_bytes(bytes);
            }
            return;
        }

        if event.button == MouseButton::Left {
            let cell = (!self.vt.is_sync_held()).then_some((col, row));
            self.vt.selection_release(cell);
            cx.notify();
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (_, _, lx, ly) = self.pointer_to_grid(event.position);
        let delta_px = f32::from(event.delta.pixel_delta(px(self.last_row_h)).y);
        self.scroll_px_accum += delta_px;
        let step_px = self.last_row_h.max(1.0);
        let lines = (self.scroll_px_accum / step_px).trunc() as isize;
        if lines == 0 {
            return;
        }
        self.scroll_px_accum -= lines as f32 * step_px;

        if self.vt.is_mouse_tracking() && !event.modifiers.shift {
            let (x, y) = self.mouse_encoder_position(lx, ly);
            let btn = if lines > 0 {
                VtMouseButton::Four
            } else {
                VtMouseButton::Five
            };
            let count = lines.unsigned_abs().min(10);
            let mods = input::gpui_mods_to_ghostty(&event.modifiers);
            let sz = self.mouse_encoder_size();
            for _ in 0..count {
                let bytes =
                    self.vt
                        .encode_mouse(VtMouseAction::Press, Some(btn), mods, x, y, false, sz);
                self.send_user_bytes(bytes);
            }
            return;
        }

        if self.vt.active_screen() == Screen::Alternate && self.vt.is_alternate_scroll_enabled() {
            let key = if lines > 0 {
                VtKey::ArrowUp
            } else {
                VtKey::ArrowDown
            };
            let count = lines.unsigned_abs().min(10);
            for _ in 0..count {
                let seq = self.vt.encode_key(key, VtMods::empty(), None, None);
                self.send_user_bytes(seq);
            }
            return;
        }

        // Positive pixel delta (scroll up on macOS) -> negative Ghostty ScrollViewport::Delta
        self.vt.scroll_viewport(ScrollViewport::Delta(-lines));
        cx.notify();
    }

    fn current_ime_font(&self) -> Font {
        build_terminal_font(self.style.preset, self.style.nerd_font_fallback)
    }

    fn sync_resize_if_needed(
        &mut self,
        content_bounds: Bounds<Pixels>,
        cell_w: f32,
        row_h: f32,
    ) -> bool {
        if self.startup_task.is_some() {
            return false;
        }
        let usable_w = f32::from(content_bounds.size.width).max(cell_w);
        let usable_h = f32::from(content_bounds.size.height).max(row_h);
        let new_cols = ((usable_w / cell_w).floor() as u16).clamp(1, MAX_GRID_DIMENSION);
        let new_rows = ((usable_h / row_h).floor() as u16).clamp(1, MAX_GRID_DIMENSION);
        let cell_w_u32 = (cell_w.round() as u32).max(1);
        let cell_h_u32 = (row_h.round() as u32).max(1);

        let grid = (new_cols, new_rows);
        let current = (self.vt.cols(), self.vt.rows());
        // Only a new local layout request may override the core's grid. Pixel
        // metrics still need to reach VT even if the core's grid stays unchanged.
        let pty_grid = (self.last_requested_grid != Some(grid) && grid != current).then_some(grid);
        if pty_grid.is_none() && self.vt.cell_pixel_size() == Some((cell_w_u32, cell_h_u32)) {
            self.last_requested_grid = Some(grid);
            return false;
        }

        let (cols, rows) = pty_grid.unwrap_or(current);
        let mut resize = || self.vt.resize(cols, rows, cell_w_u32, cell_h_u32);
        let result = match &self.herdr_handle {
            Some(handle) => handle.try_update_size_with_reports(pty_grid, resize),
            None => resize().map(|_| true),
        };
        match result {
            Ok(true) => {
                self.last_requested_grid = Some(grid);
                if pty_grid.is_some() {
                    self.cache.clear();
                }
            }
            Ok(false) => return true,
            Err(error) => self.session_error = Some(error),
        }
        false
    }
}

fn stream_closed(this: &gpui::WeakEntity<LiveTerminalView>, cx: &mut gpui::AsyncApp) -> bool {
    this.read_with(cx, |view, _| view.session_closed.is_some())
        .unwrap_or(true)
}

fn should_route_mouse_move_to_pty(
    pressed_button: Option<MouseButton>,
    mouse_presses: &[(MouseButton, bool)],
    mouse_tracking: bool,
    shift: bool,
    content_bounds: Option<Bounds<Pixels>>,
    position: Point<Pixels>,
) -> bool {
    match pressed_button {
        Some(button) => mouse_presses.contains(&(button, true)),
        None => {
            mouse_tracking
                && !shift
                && content_bounds.is_some_and(|bounds| bounds.contains(&position))
        }
    }
}

fn gpui_button_to_vt(btn: MouseButton) -> Option<VtMouseButton> {
    match btn {
        MouseButton::Left => Some(VtMouseButton::Left),
        MouseButton::Right => Some(VtMouseButton::Right),
        MouseButton::Middle => Some(VtMouseButton::Middle),
        _ => None,
    }
}

impl Focusable for LiveTerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for LiveTerminalView {
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
        if let Some(committed) = self.ime.replace_text_in_range(range_utf16, text) {
            if self.vt.has_selection() {
                self.vt.clear_selection();
            }
            self.send_user_bytes(committed.into_bytes());
        }
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
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let anchor = self.last_ime_anchor.unwrap_or(element_bounds);
        let row_h = px(self.last_row_h.max(1.0));
        let preedit = self.ime.preedit_text();
        if !preedit.is_empty() {
            let fresh_line;
            let (line, left) = if let Some((overlay, line)) = self.last_ime_overlay.as_ref()
                && line.text.as_ref() == preedit
            {
                (line, overlay.left())
            } else {
                fresh_line = window.text_system().shape_line(
                    SharedString::from(preedit.to_owned()),
                    px(self.style.font_size),
                    &[TextRun {
                        len: preedit.len(),
                        font: self.current_ime_font(),
                        color: rgb(FG_PRIMARY).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                let left = self.last_content_bounds.map_or(anchor.left(), |bounds| {
                    anchor
                        .left()
                        .min((bounds.right() - fresh_line.width - px(4.0)).max(bounds.left()))
                });
                (&fresh_line, left)
            };
            let range = self.ime.range_from_utf16(&range_utf16);
            let start_x = line.x_for_index(range.start);
            let end_x = line.x_for_index(range.end);
            return Some(Bounds::from_corners(
                point(left + start_x, anchor.top()),
                point(left + end_x.max(start_x + px(2.0)), anchor.top() + row_h),
            ));
        }
        Some(Bounds::new(
            anchor.origin,
            size(anchor.size.width.max(px(2.0)), row_h),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let (anchor, line) = self
            .last_ime_overlay
            .as_ref()
            .filter(|(_, line)| line.text.as_ref() == self.ime.preedit_text())?;
        let local_x = (point.x - anchor.left()).max(px(0.0));
        let utf8_idx = line.closest_index_for_x(local_x);
        Some(self.ime.offset_to_utf16(utf8_idx))
    }
}

/// Custom GPUI `InputHandler` wrapper opting out of macOS press-and-hold accent picker
/// so key repeats (e.g. `j`/`k` in Neovim) work natively like Ghostty/iTerm2.
struct TerminalInputHandler {
    inner: ElementInputHandler<LiveTerminalView>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.inner
            .selected_text_range(ignore_disabled_input, window, cx)
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.inner.marked_text_range(window, cx)
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.inner
            .text_for_range(range_utf16, adjusted_range, window, cx)
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .replace_text_in_range(replacement_range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.replace_and_mark_text_in_range(
            range_utf16,
            new_text,
            new_selected_range,
            window,
            cx,
        );
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.inner.unmark_text(window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.inner.bounds_for_range(range_utf16, window, cx)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.inner.character_index_for_point(point, window, cx)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}

struct LiveTerminalElement {
    view: Entity<LiveTerminalView>,
}

struct LiveTerminalPrepaint {
    prepared: Option<std::rc::Rc<PreparedTerminalFrame>>,
    content_bounds: Bounds<Pixels>,
    hitbox: gpui::Hitbox,
}

impl IntoElement for LiveTerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LiveTerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LiveTerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("live-terminal-element".into()))
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
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let content_bounds = Bounds::new(
            point(
                bounds.left() + px(TERM_PAD_X),
                bounds.top() + px(TERM_PAD_Y),
            ),
            size(
                (bounds.size.width - px(TERM_PAD_X * 2.0)).max(px(16.0)),
                (bounds.size.height - px(TERM_PAD_Y * 2.0)).max(px(18.0)),
            ),
        );

        let scale_factor = window.scale_factor();
        let style_cfg = self.view.read(cx).style.clone();
        let metrics = measure_cell_metrics(window.text_system(), &style_cfg);
        let cell_w = f32::from(metrics.cell_width);
        let row_h = f32::from(metrics.row_height);

        let prepared = self.view.update(cx, |view, _| {
            view.last_content_bounds = Some(content_bounds);
            if view.sync_resize_if_needed(content_bounds, cell_w, row_h) {
                let view = self.view.downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = view.update(cx, |_, cx| cx.notify());
                });
            }
            if !view.vt.is_sync_held()
                && let Ok(frame) = view.cache.prepare_frame(
                    &mut view.vt,
                    &view.style,
                    scale_factor,
                    window.text_system(),
                )
            {
                view.last_cell_w = f32::from(frame.metrics.cell_width);
                view.last_row_h = f32::from(frame.metrics.row_height);
                view.presented_frame = Some(std::rc::Rc::new(frame));
            }
            view.presented_frame.clone()
        });

        LiveTerminalPrepaint {
            prepared,
            content_bounds,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (
            focus_handle,
            window_active,
            cursor_blink_on,
            is_mouse_tracking,
            ime_preedit,
            ime_marked,
            ime_selected,
            ime_font,
            ime_font_size,
        ) = {
            let v = self.view.read(cx);
            (
                v.focus_handle.clone(),
                v.window_active,
                v.cursor_blink_on,
                v.vt.is_mouse_tracking(),
                v.ime.preedit_text().to_string(),
                v.ime.marked_range.clone(),
                v.ime.selected_range.clone(),
                v.current_ime_font(),
                v.style.font_size,
            )
        };

        window.set_cursor_style(
            if is_mouse_tracking {
                gpui::CursorStyle::Arrow
            } else {
                gpui::CursorStyle::IBeam
            },
            &prepaint.hitbox,
        );

        let inner_handler = ElementInputHandler::new(prepaint.content_bounds, self.view.clone());
        window.handle_input(
            &focus_handle,
            TerminalInputHandler {
                inner: inner_handler,
            },
            cx,
        );

        let Some(frame) = prepaint.prepared.as_ref() else {
            return;
        };
        let ime_overlay = if ime_preedit.is_empty() {
            None
        } else {
            Some(ImeOverlayState {
                preedit: &ime_preedit,
                marked_range: ime_marked,
                selected_range: ime_selected,
                font: ime_font,
                font_size: ime_font_size,
            })
        };

        let (cursor_anchor, ime_overlay) = paint_terminal_frame(
            frame,
            prepaint.content_bounds,
            window_active,
            cursor_blink_on,
            ime_overlay,
            window,
            cx,
        );

        let cursor_blinking = frame.meta.cursor.visible && frame.meta.cursor.blinking;
        self.view.update(cx, |view, cx| {
            view.last_ime_anchor = Some(cursor_anchor);
            view.last_ime_overlay = ime_overlay;
            view.maybe_arm_cursor_blink(cursor_blinking, cx);
        });
    }
}

impl Render for LiveTerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(title) = self.pending_title.take() {
            window.set_window_title(&title);
        }

        let mut root = div()
            .id("live-terminal-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .capture_any_mouse_up(cx.listener(Self::on_mouse_up))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG_PRIMARY))
            .text_color(rgb(FG_PRIMARY));

        if !self.config.minimal {
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .bg(rgb(BG_SURFACE))
                    .border_b_1()
                    .border_color(rgb(BORDER_SUBTLE))
                    .text_size(px(11.5))
                    .children(FontPreset::ALL.into_iter().map(|preset| {
                        let selected = self.style.preset == preset;
                        div()
                            .id(SharedString::from(format!(
                                "live-preset-{}",
                                preset.label()
                            )))
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(if selected { ACCENT_BLUE } else { BORDER_SUBTLE }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.style.preset = preset;
                                    this.cache.clear();
                                    window.focus(&this.focus_handle);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                            .child(preset.label())
                    }))
                    .child(
                        div()
                            .id("live-toggle-nf")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(if self.style.nerd_font_fallback {
                                ACCENT_GREEN
                            } else {
                                BORDER_SUBTLE
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.style.nerd_font_fallback = !this.style.nerd_font_fallback;
                                    this.cache.clear();
                                    window.focus(&this.focus_handle);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                            .child(if self.style.nerd_font_fallback {
                                self.locale.label("Nerd Font: on", "Nerd Font: 有効")
                            } else {
                                self.locale.label("Nerd Font: off", "Nerd Font: 無効")
                            }),
                    ),
            );
        }

        root = root.child(
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
                .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
                .child(LiveTerminalElement {
                    view: cx.entity().clone(),
                }),
        );

        let restart_button = || {
            session_button("live-restart-btn", self.locale.label("Restart", "再起動"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.start_session(cx);
                        window.focus(&this.focus_handle);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
        };
        if let Some(info) = &self.session_closed {
            let can_reconnect = info.reason == "disconnected" && self.herdr_guard.is_some();
            let summary = info.summary(self.locale == locale::Locale::Japanese);
            let closed_label = self.locale.label("Session closed", "セッション終了");
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_1p5()
                    .bg(rgb(BG_SURFACE))
                    .border_t_1()
                    .border_color(rgb(ACCENT_RED))
                    .text_size(px(12.0))
                    .child(format!("{closed_label}: {summary}"))
                    .when(can_reconnect, |row| {
                        row.child(
                            session_button(
                                "live-reconnect-btn",
                                self.locale.label("Reconnect", "再接続"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.reattach(cx);
                                    window.focus(&this.focus_handle);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                        )
                    })
                    .when(!can_reconnect || self.session_error.is_some(), |row| {
                        row.child(restart_button())
                    }),
            );
        }
        if self.startup_task.is_some() {
            root = root.child(
                div()
                    .px_3()
                    .py_1p5()
                    .text_size(px(12.0))
                    .child(self.locale.label(
                        "Starting terminal… Input is unavailable.",
                        "端末を起動中です。起動が完了すると入力できます。",
                    )),
            );
        }
        if let Some(err) = &self.session_error {
            root = root.child(
                div()
                    .px_3()
                    .py_1p5()
                    .bg(rgb(BG_SURFACE))
                    .border_t_1()
                    .border_color(rgb(ACCENT_RED))
                    .text_size(px(12.0))
                    .child(format!(
                        "{}: {err}",
                        self.locale.label("herdr-core error", "herdr-core エラー")
                    ))
                    .when(
                        self.session_closed.is_none() && self.herdr_guard.is_none(),
                        |row| row.child(restart_button()),
                    ),
            );
        }

        root
    }
}

// ---------------------------------------------------------------------------
// Stage 1/2 Probe UI + Stage 2/3 Window Benchmarks (shared RenderStyle/Cache)
// ---------------------------------------------------------------------------

fn session_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_sm()
        .cursor_pointer()
        .bg(rgb(ACCENT_BLUE))
        .child(label)
}

fn main() {
    let raw_os_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if raw_os_args
        .get(1)
        .is_some_and(|a| a == os::PTY_IUTF8_INIT_FLAG)
    {
        os::run_pty_iutf8_init(&raw_os_args[2..]);
    }

    if raw_os_args
        .get(1)
        .is_some_and(|a| a == herdr::process::SUPERVISOR_FLAG)
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("start core supervisor runtime");
        if let Err(error) = runtime.block_on(herdr::process::run_supervisor()) {
            eprintln!("uiterm: {error}");
            std::process::exit(1);
        }
        return;
    }

    let args: Vec<String> = std::env::args().collect();
    let cfg = CliConfig::parse(&args);
    if cfg.mode == RunMode::BenchVt {
        vt::run_headless_vt_benchmark();
        return;
    }

    let signal_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_io()
        .build()
        .expect("start signal cleanup runtime");
    let quit_signal =
        install_signal_cleanup_watcher(&signal_runtime).expect("install signal cleanup watcher");

    let started_at = Instant::now();

    Application::new().run(move |cx: &mut App| {
        #[cfg(target_os = "linux")]
        fonts::init_linux_font_families(cx.text_system().all_font_names());

        {
            let exit_signal = std::rc::Rc::new(std::cell::Cell::new(None));
            cx.on_app_quit({
                let exit_signal = exit_signal.clone();
                move |_| {
                    let signal = exit_signal.get();
                    async move {
                        // GPUI polls shutdown futures after running the synchronous cleanup callbacks.
                        if let Some(signal) = signal {
                            unsafe {
                                libc::signal(signal, libc::SIG_DFL);
                                libc::raise(signal);
                            }
                        }
                    }
                }
            })
            .detach();
            cx.spawn(async move |cx| {
                if let Ok(signal) = quit_signal.await {
                    exit_signal.set(Some(signal));
                    let _ = cx.update(|cx| cx.quit());
                }
            })
            .detach();
        }

        if cfg.mode == RunMode::ProbeUi {
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
        } else if cfg.mode != RunMode::LiveTerminal {
            cx.bind_keys([KeyBinding::new("cmd-q", QuitPoc, None)]);
            cx.on_action(|_: &QuitPoc, cx| cx.quit());
        }

        let est_cell_w = cfg.font_size * 0.6025;
        let est_row_h = (cfg.font_size * DEFAULT_LINE_HEIGHT).ceil();
        let win_size = match cfg.mode {
            RunMode::LiveTerminal => {
                let w = (cfg.cols as f32 * est_cell_w + TERM_PAD_X * 2.0 + 4.0).ceil();
                let toolbar_h = if cfg.minimal { 0.0 } else { 30.0 };
                let h = (cfg.rows as f32 * est_row_h + TERM_PAD_Y * 2.0 + toolbar_h).ceil();
                size(px(w), px(h))
            }
            RunMode::BenchWindow => {
                let w = (cfg.cols as f32 * est_cell_w + TERM_PAD_X * 2.0 + 4.0).ceil();
                let h = (cfg.rows as f32 * est_row_h + TERM_PAD_Y * 2.0).ceil();
                size(px(w), px(h))
            }
            RunMode::ProbeUi | RunMode::ProbeFonts => size(px(920.0), px(420.0)),
            RunMode::BenchVt => unreachable!("headless benchmark runs before GPUI"),
        };

        let bounds = Bounds::centered(None, win_size, cx);
        let window_opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };

        let cfg_clone = cfg.clone();
        match cfg.mode {
            RunMode::LiveTerminal => {
                cx.open_window(window_opts, |window, cx| {
                    cx.new(|cx| LiveTerminalView::new(cfg_clone, window, cx))
                })
                .expect("open live GPUI terminal window");
            }
            _ => {
                cx.open_window(window_opts, |window, cx| {
                    cx.new(|cx| GpuiPocView::new(&cfg_clone, started_at, window, cx))
                })
                .expect("open GPUI PoC window");
            }
        }
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_watcher_delivers_sigterm_and_sigint_in_an_isolated_process() {
        const CHILD: &str = "UITERM_TEST_SIGNAL_WATCHER";
        if std::env::var_os(CHILD).is_some() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            for sig in [libc::SIGTERM, libc::SIGINT] {
                let received = install_signal_cleanup_watcher(&runtime).unwrap();
                assert_eq!(unsafe { libc::raise(sig) }, 0);
                assert_eq!(
                    runtime
                        .block_on(async {
                            tokio::time::timeout(Duration::from_secs(2), received).await
                        })
                        .unwrap()
                        .unwrap(),
                    sig,
                );
            }
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::signal_watcher_delivers_sigterm_and_sigint_in_an_isolated_process",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn command_line_dimensions_and_history_are_bounded() {
        let args = [
            "uiterm",
            "--cols",
            "65535",
            "--rows",
            "0",
            "--scrollback-lines",
            "100000",
            "--font-size",
            "NaN",
        ]
        .map(str::to_owned);
        let config = CliConfig::parse(&args);
        assert_eq!((config.cols, config.rows), (MAX_GRID_DIMENSION, 1));
        assert_eq!(config.scrollback_lines, MAX_SCROLLBACK_LINES);
        assert_eq!(config.font_size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn benchmark_flag_is_not_consumed_from_child_arguments() {
        let args = ["uiterm", "--cmd", "example", "--bench-vt"].map(str::to_owned);
        let config = CliConfig::parse(&args);
        assert_eq!(config.mode, RunMode::LiveTerminal);
        assert_eq!(config.cmd_argv.unwrap(), ["example", "--bench-vt"]);
        let args = ["uiterm", "--bench-vt"].map(str::to_owned);
        assert_eq!(CliConfig::parse(&args).mode, RunMode::BenchVt);
    }

    #[test]
    fn mouse_move_ignores_toolbar_hover_and_button_drags() {
        let content_bounds = Some(Bounds::new(
            point(px(8.0), px(34.0)),
            size(px(800.0), px(500.0)),
        ));
        let toolbar_pos = point(px(40.0), px(10.0));
        let terminal_pos = point(px(40.0), px(60.0));

        assert!(!should_route_mouse_move_to_pty(
            None,
            &[],
            true,
            false,
            content_bounds,
            toolbar_pos,
        ));
        assert!(should_route_mouse_move_to_pty(
            None,
            &[],
            true,
            false,
            content_bounds,
            terminal_pos,
        ));
        assert!(!should_route_mouse_move_to_pty(
            None,
            &[],
            true,
            true,
            content_bounds,
            terminal_pos,
        ));
        assert!(should_route_mouse_move_to_pty(
            Some(MouseButton::Left),
            &[(MouseButton::Left, true)],
            true,
            false,
            content_bounds,
            toolbar_pos,
        ));
        assert!(!should_route_mouse_move_to_pty(
            Some(MouseButton::Left),
            &[],
            true,
            false,
            content_bounds,
            terminal_pos,
        ));
    }
}
