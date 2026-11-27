mod fonts;
mod herdr;
mod ime;
mod input;
mod os;
mod renderer;
mod vt;

use std::ops::Range;
use std::path::PathBuf;
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
use herdr::{CloseInfo, HerdrGuard, HerdrHandle, HerdrOutputStream, HerdrStartOptions, SessionOutput};
use ime::{ImeBuffer, TerminalImeState};
use input::{KeyDispatchDecision, TerminalAppAction, classify_keydown};
use renderer::{
    ImeOverlayState, PreparedTerminalFrame, RenderCacheMode, RenderStyleConfig, RowShapeCache,
    measure_cell_metrics, paint_terminal_frame,
};
use vt::{DEFAULT_SCROLLBACK_LINES, VtTerminal, current_rss_mib, make_10k_payload};

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

#[cfg(unix)]
static mut SIGNAL_PIPE_WR: libc::c_int = -1;

#[cfg(unix)]
extern "C" fn handle_term_signal(sig: libc::c_int) {
    unsafe {
        if SIGNAL_PIPE_WR >= 0 {
            let b = sig as u8;
            let _ = libc::write(SIGNAL_PIPE_WR, (&raw const b).cast(), 1);
        }
    }
}

#[cfg(unix)]
fn install_signal_cleanup_watcher() -> Option<tokio::sync::oneshot::Receiver<u8>> {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    let read_fd = fds[0];
    let write_fd = fds[1];
    unsafe {
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        libc::fcntl(write_fd, libc::F_SETFL, libc::O_NONBLOCK);
        SIGNAL_PIPE_WR = write_fd;
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handle_term_signal as *const () as usize;
        libc::sigemptyset(&raw mut sa.sa_mask);
        sa.sa_flags = libc::SA_RESTART;
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("sig-cleanup".into())
        .spawn(move || {
            let mut signal = 0u8;
            if unsafe { libc::read(read_fd, (&raw mut signal).cast(), 1) } > 0 {
                let _ = tx.send(signal);
            }
        })
        .expect("start signal cleanup watcher");
    Some(rx)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunMode {
    LiveTerminal,
    ProbeUi,
    ProbeFonts,
    BenchWindow,
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
                "--minimal" => minimal = true,
                "--no-iutf8-wrap" => wrap_iutf8 = false,
                "--cols" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cols = v;
                        i += 1;
                    }
                }
                "--rows" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        rows = v;
                        i += 1;
                    }
                }
                "--font-size" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<f32>().ok()) {
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
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        scrollback_lines = v;
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
    session_error: Option<String>,
    window_active: bool,
    cursor_blink_on: bool,
    cursor_blink_armed: bool,
    sync_timer_armed: bool,
    pending_title: Option<String>,
    last_content_bounds: Option<Bounds<Pixels>>,
    last_requested_grid: Option<(u16, u16)>,
    last_cell_w: f32,
    last_row_h: f32,
    last_ime_anchor: Option<Bounds<Pixels>>,
    last_ime_line: Option<ShapedLine>,
    scroll_px_accum: f32,
    first_paint_logged: bool,
    started_at: Instant,
    _output_task: Option<gpui::Task<()>>,
}

impl LiveTerminalView {
    fn new(
        config: CliConfig,
        started_at: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
            session_error: None,
            window_active: window.is_window_active(),
            cursor_blink_on: true,
            cursor_blink_armed: false,
            sync_timer_armed: false,
            pending_title: Some("uiterm GPUI PoC".to_string()),
            last_content_bounds: None,
            last_requested_grid: None,
            last_cell_w: 8.43,
            last_row_h: (config.font_size * DEFAULT_LINE_HEIGHT).ceil(),
            last_ime_anchor: None,
            last_ime_line: None,
            scroll_px_accum: 0.0,
            first_paint_logged: false,
            started_at,
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
        self.shutdown();
        self.session_closed = None;
        self.session_error = None;
        self.vt = VtTerminal::new(
            self.vt.cols(),
            self.vt.rows(),
            self.config.scrollback_lines,
        )
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
            tag: "live".to_string(),
            argv: Some(argv),
            cwd: None,
            env,
            cols: self.vt.cols(),
            rows: self.vt.rows(),
            history_lines: self.config.scrollback_lines,
            wrap_iutf8: self.config.wrap_iutf8,
        };

        match HerdrGuard::start_live(opts) {
            Ok((guard, handle, stream)) => {
                self.herdr_guard = Some(guard);
                self.start_output_pump(handle, stream, cx);
            }
            Err(err) => self.session_error = Some(err),
        }
    }

    fn start_output_pump(&mut self, handle: HerdrHandle, mut stream: HerdrOutputStream, cx: &mut Context<Self>) {
        self.session_error = handle.try_send_focus(self.window_active).err();
        self.herdr_handle = Some(handle);
        self._output_task = Some(cx.spawn(async move |this, cx| {
            Self::pump_pty_output(this, &mut stream, cx).await;
        }));
    }

    fn reattach(&mut self, cx: &mut Context<Self>) {
        let Some(guard) = &mut self.herdr_guard else { return };
        self._output_task = None;
        self.herdr_handle = None;
        let (cols, rows) = self.last_requested_grid.unwrap_or((self.vt.cols(), self.vt.rows()));
        let opts = HerdrStartOptions {
            cols,
            rows,
            history_lines: self.config.scrollback_lines,
            ..HerdrStartOptions::default()
        };
        match guard.reattach(opts) {
            Ok((handle, stream)) => {
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
            TerminalAppAction::ToggleRenderCache => {
                self.cache.mode = match self.cache.mode {
                    RenderCacheMode::DirtyRowCache => RenderCacheMode::Uncached,
                    RenderCacheMode::Uncached => RenderCacheMode::DirtyRowCache,
                };
                self.cache.clear();
                cx.notify();
            }
            TerminalAppAction::Quit => {
                self.shutdown();
                cx.quit();
            }
        }
    }

    fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            KeyDispatchDecision::CancelStuckImeMark => {
                if self.ime.cancel_mark_if_any() {
                    cx.notify();
                }
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
                let seq = self.vt.encode_key(key, mods, utf8.as_deref(), unshifted_codepoint);
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
        let col = ((local_x / cw).floor() as u16).min(self.vt.cols().saturating_sub(1));
        let row = ((local_y / rh).floor() as u16).min(self.vt.rows().saturating_sub(1));
        (col, row, local_x as f64, local_y as f64)
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
            .map(|b| (f32::from(b.size.width).round() as u32, f32::from(b.size.height).round() as u32))
            .unwrap_or((800, 500));
        let cell_width = (self.last_cell_w.round() as u32).max(1);
        let cell_height = (self.last_row_h.round() as u32).max(1);
        let (screen_width, screen_height) = if self.vt.uses_pixel_mouse_coordinates() {
            (w.max(1), h.max(1))
        } else {
            (u32::from(self.vt.cols()) * cell_width, u32::from(self.vt.rows()) * cell_height)
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

        if self.vt.is_mouse_tracking() && !event.modifiers.shift {
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
                self.send_user_bytes(bytes);
            }
            return;
        }

        if event.button == MouseButton::Left {
            self.vt.selection_press(col, row, lx, ly);
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
        if self.vt.is_mouse_tracking() && !event.modifiers.shift {
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

        if event.pressed_button == Some(MouseButton::Left) {
            let bounds = self.last_content_bounds.unwrap_or_default();
            self.vt.selection_drag(
                col,
                row,
                (lx, ly),
                event.modifiers.alt,
                (
                    (self.last_cell_w.round() as u32).max(1),
                    0,
                    (f32::from(bounds.size.height).round() as u32).max(1),
                ),
            );
            cx.notify();
        }
    }

    fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (col, row, lx, ly) = self.pointer_to_grid(event.position);
        if self.vt.is_mouse_tracking() && !event.modifiers.shift {
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
            self.vt.selection_release(col, row);
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
                let bytes = self.vt.encode_mouse(
                    VtMouseAction::Press,
                    Some(btn),
                    mods,
                    x,
                    y,
                    false,
                    sz,
                );
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

    fn sync_resize_if_needed(&mut self, content_bounds: Bounds<Pixels>, cell_w: f32, row_h: f32) -> bool {
        let usable_w = f32::from(content_bounds.size.width).max(cell_w);
        let usable_h = f32::from(content_bounds.size.height).max(row_h);
        let new_cols = (usable_w / cell_w).floor().max(1.0) as u16;
        let new_rows = (usable_h / row_h).floor().max(1.0) as u16;
        let cell_w_u32 = (cell_w.round() as u32).max(1);
        let cell_h_u32 = (row_h.round() as u32).max(1);

        let grid = (new_cols, new_rows);
        // The server can change its grid independently. Only a local layout change
        // issues a new resize, so processing a remote notification cannot echo it.
        if self.last_requested_grid == Some(grid) {
            return false;
        }
        if grid == (self.vt.cols(), self.vt.rows()) {
            self.last_requested_grid = Some(grid);
            return false;
        }

        let mut resize = || self.vt.resize(new_cols, new_rows, cell_w_u32, cell_h_u32);
        let result = match &self.herdr_handle {
            Some(handle) => handle.try_resize_with_reports(new_cols, new_rows, resize),
            None => resize().map(|_| true),
        };
        match result {
            Ok(true) => {
                self.last_requested_grid = Some(grid);
                self.cache.clear();
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
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let anchor = self.last_ime_anchor.unwrap_or(element_bounds);
        let row_h = px(self.last_row_h.max(1.0));
        if let Some(line) = self.last_ime_line.as_ref()
            && !line.text.is_empty()
        {
            let range = self.ime.range_from_utf16(&range_utf16);
            let start_x = line.x_for_index(range.start.min(line.text.len()));
            let end_x = line.x_for_index(range.end.min(line.text.len()));
            return Some(Bounds::from_corners(
                point(anchor.left() + start_x, anchor.top()),
                point(
                    anchor.left() + end_x.max(start_x + px(2.0)),
                    anchor.top() + row_h,
                ),
            ));
        }
        Some(Bounds::new(anchor.origin, size(px(self.last_cell_w.max(2.0)), row_h)))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let anchor = self.last_ime_anchor?;
        let line = self.last_ime_line.as_ref()?;
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
            point(bounds.left() + px(TERM_PAD_X), bounds.top() + px(TERM_PAD_Y)),
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
            view.last_cell_w = cell_w;
            view.last_row_h = row_h;
            if view.sync_resize_if_needed(content_bounds, cell_w, row_h) {
                let view = self.view.downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = view.update(cx, |_, cx| cx.notify());
                });
            }
            if !view.vt.is_sync_held()
                && let Ok(frame) = view.cache.prepare_frame(&mut view.vt, &view.style, scale_factor, window.text_system())
            {
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
        let (focus_handle, window_active, cursor_blink_on, is_mouse_tracking, ime_preedit, ime_marked, ime_selected, ime_font, ime_font_size) = {
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

        let ime_result = paint_terminal_frame(
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
            if let Some((anchor, line)) = ime_result {
                view.last_ime_anchor = Some(anchor);
                view.last_ime_line = Some(line);
            }
            if !view.first_paint_logged {
                view.first_paint_logged = true;
                if std::env::var_os("UITERM_GPUI_LOG_STARTUP").is_some() {
                    println!(
                        "GPUI_FIRST_PAINT_MS={:.2}",
                        view.started_at.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
            view.maybe_arm_cursor_blink(cursor_blinking, cx);
        });
    }
}

impl Render for LiveTerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(title) = self.pending_title.take() {
            window.set_window_title(&title);
        }

        let stats = self.cache.last_stats;
        let header_status = format!(
            "{}x{} | {} | {:.1}px | shape={} hit/miss={}/{} | {}us | {:.1} KiB",
            self.vt.cols(),
            self.vt.rows(),
            self.cache.mode.label(),
            self.style.font_size,
            stats.shape_calls,
            stats.cache_hits,
            stats.cache_misses,
            stats.prepaint_us,
            stats.estimated_cache_bytes as f64 / 1024.0
        );

        let mut root = div()
            .id("live-terminal-root")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
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
                            .id(SharedString::from(format!("live-preset-{}", preset.label())))
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
                                    cx.notify();
                                }),
                            )
                            .child(if self.style.nerd_font_fallback {
                                "NF: ON"
                            } else {
                                "NF: OFF"
                            }),
                    )
                    .child(
                        div()
                            .id("live-toggle-cache")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(
                                if self.cache.mode == RenderCacheMode::DirtyRowCache {
                                    ACCENT_CYAN
                                } else {
                                    BORDER_SUBTLE
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.cache.mode = match this.cache.mode {
                                        RenderCacheMode::DirtyRowCache => RenderCacheMode::Uncached,
                                        RenderCacheMode::Uncached => RenderCacheMode::DirtyRowCache,
                                    };
                                    this.cache.clear();
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }),
                            )
                            .child(format!("Cache: {}", self.cache.mode.label())),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_color(rgb(FG_MUTED))
                            .child(header_status),
                    ),
            );
        }

        root = root.child(
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(LiveTerminalElement {
                    view: cx.entity().clone(),
                }),
        );

        if let Some(info) = &self.session_closed {
            let can_reconnect = info.reason == "disconnected" && self.session_error.is_none();
            let summary = info.summary();
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
                    .child(format!("Session closed: {summary}"))
                    .child(
                        div()
                            .id("live-restart-btn")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(ACCENT_BLUE))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    if can_reconnect {
                                        this.reattach(cx);
                                    } else {
                                        this.start_session(cx);
                                    }
                                    window.focus(&this.focus_handle);
                                    cx.notify();
                                }),
                            )
                            .child(if can_reconnect { "Reconnect" } else { "Restart" }),
                    ),
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
                    .child(format!("herdr-core error: {err}")),
            );
        }

        root
    }
}

// ---------------------------------------------------------------------------
// Stage 1/2 Probe UI + Stage 2/3 Window Benchmarks (shared RenderStyle/Cache)
// ---------------------------------------------------------------------------

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

fn feed_benchmark_output(
    rt: &tokio::runtime::Runtime,
    stream: &HerdrOutputStream,
    vt: &mut VtTerminal,
    bytes: &[u8],
) {
    let replies = vt.feed(bytes).pty_writes;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), stream.acknowledge(bytes.len() as u32, replies))
            .await
    })
    .expect("benchmark writer capacity timed out")
    .expect("benchmark writer closed");
}

struct BenchState {
    scenario: BenchScenario,
    started_at: Instant,
    first_paint_ms: Option<f64>,
    herdr_guard: Option<HerdrGuard>,
    herdr_handle: Option<HerdrHandle>,
    herdr_stream: Option<HerdrOutputStream>,
    vt: VtTerminal,
    cache: RowShapeCache,
    style: RenderStyleConfig,
    frame_times_ms: Vec<f64>,
    prepaint_us_samples: Vec<u64>,
    shape_calls_total: usize,
    dirty_rows_total: usize,
    cache_hits_total: usize,
    cache_misses_total: usize,
    max_cached_entries: usize,
    max_cache_bytes: usize,
    last_frame_at: Option<Instant>,
    total_frames: usize,
    aux_metric_ms: f64,
    aux_bytes: usize,
    latencies_ms: Vec<f64>,
    rt: tokio::runtime::Runtime,
    bench_finished: bool,
    tmp_file: Option<PathBuf>,
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
    fn new(
        cfg: &CliConfig,
        started_at: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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

        cx.on_app_quit(|this, _cx| {
            if let Some(bench) = &mut this.bench
                && let Some(mut guard) = bench.herdr_guard.take()
            {
                guard.stop_blocking();
            }
            std::future::ready(())
        }).detach();

        let bench = if cfg.mode == RunMode::BenchWindow {
            Some(Self::init_bench_state(cfg, started_at))
        } else {
            None
        };

        Self {
            focus_handle,
            preset: cfg.font_preset,
            nerd_font_fallback: true,
            font_size: cfg.font_size,
            ime: ImeBuffer::new(),
            last_ime_bounds: None,
            last_ime_line: None,
            mode: cfg.mode,
            probe_done: false,
            window_active: true,
            bench,
        }
    }

    fn init_bench_state(cfg: &CliConfig, started_at: Instant) -> BenchState {
        let style = RenderStyleConfig {
            preset: cfg.font_preset,
            nerd_font_fallback: true,
            font_size: cfg.font_size,
            line_height: DEFAULT_LINE_HEIGHT,
        };
        let mut vt =
            VtTerminal::new(cfg.cols, cfg.rows, cfg.scrollback_lines).expect("vt new");
        let cache = RowShapeCache::new(cfg.render_mode);
        let mut aux_metric_ms = 0.0;
        let mut aux_bytes = 0usize;
        let mut tmp_file: Option<PathBuf> = None;

        let (herdr_guard, herdr_handle, mut herdr_stream) = match cfg.scenario {
            BenchScenario::NvimCtrlDu => {
                let sample_path =
                    PathBuf::from(format!("/tmp/uiterm-nvim-bench-{}.rs", std::process::id()));
                let mut content = String::with_capacity(64 * 1024);
                for i in 0..1000 {
                    content.push_str(&format!(
                        "// line {i:04}: 日本語コメントと \u{E0A0} main シンボル解析テスト\nfn sample_fn_{i:04}(arg: usize) -> usize {{ arg.saturating_add({i}) }}\n"
                    ));
                }
                std::fs::write(&sample_path, content).expect("write nvim sample");
                tmp_file = Some(sample_path.clone());

                let mut env = os::build_child_shell_env(Some("/bin/sh"));
                let qa_home = format!("/tmp/u-gp-nvhome-{}", std::process::id());
                let _ = std::fs::create_dir_all(&qa_home);
                env.insert("HOME".into(), qa_home.clone());
                env.insert("XDG_CONFIG_HOME".into(), format!("{qa_home}/cfg"));
                env.insert("XDG_STATE_HOME".into(), format!("{qa_home}/st"));
                env.insert("XDG_DATA_HOME".into(), format!("{qa_home}/data"));

                let opts = HerdrStartOptions {
                    tag: "bnvim".into(),
                    argv: Some(vec![
                        "nvim".into(),
                        "--clean".into(),
                        "-c".into(),
                        "syntax on | set number | set termguicolors | set laststatus=2".into(),
                        sample_path.to_string_lossy().into_owned(),
                    ]),
                    cwd: None,
                    env,
                    cols: cfg.cols,
                    rows: cfg.rows,
                    history_lines: cfg.scrollback_lines,
                    wrap_iutf8: true,
                };
                let (guard, handle, mut stream) = HerdrGuard::start_live(opts).expect("start nvim");
                let rt = tokio::runtime::Runtime::new().unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut seen = Vec::new();
                while Instant::now() < deadline {
                    if let Ok(Some(SessionOutput::Data(bytes))) = rt.block_on(async {
                        tokio::time::timeout(Duration::from_millis(120), stream.recv_uncredited()).await
                    }) {
                        seen.extend_from_slice(&bytes);
                        feed_benchmark_output(&rt, &stream, &mut vt, &bytes);
                        if seen.windows(b"sample_fn_0000".len()).any(|w| w == b"sample_fn_0000") {
                            break;
                        }
                    }
                }
                aux_bytes = seen.len();
                (Some(guard), Some(handle), Some(stream))
            }
            BenchScenario::Burst5MiB | BenchScenario::InputLatency => {
                let opts = HerdrStartOptions {
                    tag: "blive".into(),
                    argv: Some(vec!["/bin/sh".into()]),
                    cwd: None,
                    env: os::build_child_shell_env(Some("/bin/sh")),
                    cols: cfg.cols,
                    rows: cfg.rows,
                    history_lines: cfg.scrollback_lines,
                    wrap_iutf8: true,
                };
                let (guard, handle, stream) = HerdrGuard::start_live(opts).expect("start sh");
                (Some(guard), Some(handle), Some(stream))
            }
            BenchScenario::ScrollHistory1 | BenchScenario::ScrollHistory6 => {
                let (chunks, total_bytes) = make_10k_payload();
                for ch in &chunks {
                    vt.feed_raw(ch);
                }
                aux_bytes = total_bytes;
                (None, None, None)
            }
            BenchScenario::ScrollStream => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let opts = HerdrStartOptions {
                    tag: "bseq".into(),
                    argv: Some(vec!["/bin/sh".into()]),
                    cwd: None,
                    env: os::build_child_shell_env(Some("/bin/sh")),
                    cols: cfg.cols,
                    rows: cfg.rows,
                    history_lines: cfg.scrollback_lines,
                    wrap_iutf8: true,
                };
                let (guard, handle, mut stream) = HerdrGuard::start_live(opts).expect("start sh");
                let t_seq = Instant::now();
                let _ = handle.try_send_user(
                    b"seq 1 10000; printf '%s%s\\n' UITERM_GPUI_ BENCH_DONE\n".to_vec(),
                );
                let mut saw_done = false;
                let deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < deadline && !saw_done {
                    if let Ok(Some(SessionOutput::Data(bytes))) = rt.block_on(async {
                        tokio::time::timeout(Duration::from_millis(200), stream.recv_uncredited()).await
                    }) {
                        aux_bytes += bytes.len();
                        if bytes
                            .windows(b"UITERM_GPUI_BENCH_DONE".len())
                            .any(|w| w == b"UITERM_GPUI_BENCH_DONE")
                        {
                            saw_done = true;
                        }
                        feed_benchmark_output(&rt, &stream, &mut vt, &bytes);
                    }
                }
                aux_metric_ms = t_seq.elapsed().as_secs_f64() * 1000.0;
                (Some(guard), Some(handle), Some(stream))
            }
            BenchScenario::SingleRow => (None, None, None),
        };
        let _ = &mut herdr_stream;

        let rt = tokio::runtime::Runtime::new().unwrap();
        BenchState {
            scenario: cfg.scenario,
            started_at,
            first_paint_ms: None,
            herdr_guard,
            herdr_handle,
            herdr_stream,
            vt,
            cache,
            style,
            frame_times_ms: Vec::with_capacity(400),
            prepaint_us_samples: Vec::with_capacity(400),
            shape_calls_total: 0,
            dirty_rows_total: 0,
            cache_hits_total: 0,
            cache_misses_total: 0,
            max_cached_entries: 0,
            max_cache_bytes: 0,
            last_frame_at: None,
            total_frames: 0,
            aux_metric_ms,
            aux_bytes,
            latencies_ms: Vec::new(),
            rt,
            bench_finished: false,
            tmp_file,
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
        if bench.bench_finished {
            return;
        }
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

        let frame_idx = bench.total_frames;
        let done = match bench.scenario {
            BenchScenario::ScrollStream => {
                let line = format!(
                    "\x1b[36m[{frame_idx:05}]\x1b[0m \x1b[32mINFO\x1b[0m \u{E0A0} main 日本語スクロール描画テスト Neovim/Ctrl+D frame={frame_idx}\r\n"
                );
                bench.vt.feed_raw(line.as_bytes());
                bench.frame_times_ms.len() >= 360
            }
            BenchScenario::SingleRow => {
                if frame_idx == 0 {
                    for r in 0..bench.vt.rows().saturating_sub(1) {
                        bench.vt.feed_raw(
                            format!("row={r:02} static context 日本語 \u{E0A0} main cargo build --release\r\n")
                                .as_bytes(),
                        );
                    }
                }
                let line = format!("\r[status] step={frame_idx:05} spinner={} elapsed={}ms", frame_idx % 8, frame_idx * 16);
                bench.vt.feed_raw(line.as_bytes());
                bench.frame_times_ms.len() >= 360
            }
            BenchScenario::ScrollHistory1 | BenchScenario::ScrollHistory6 => {
                let step = if bench.scenario == BenchScenario::ScrollHistory6 {
                    6isize
                } else {
                    1isize
                };
                if frame_idx == 0 {
                    bench.vt.scroll_viewport(ScrollViewport::Bottom);
                } else {
                    let dir = if (frame_idx / 120) % 2 == 0 { -step } else { step };
                    bench.vt.scroll_viewport(ScrollViewport::Delta(dir));
                }
                bench.frame_times_ms.len() >= 360
            }
            BenchScenario::NvimCtrlDu => {
                // Send Ctrl+D or Ctrl+U across 60 cycles (120 actions), waiting for actual PTY output
                // without artificial fixed sleep, and letting GPUI render every update frame.
                if frame_idx < 120 {
                    let byte = if frame_idx % 2 == 0 { b"\x04" } else { b"\x15" };
                    let t_action = Instant::now();
                    if let Some(h) = &bench.herdr_handle {
                        let _ = h.try_send_user(byte.to_vec());
                    }
                    let mut got_output = false;
                    if let Some(stream) = &mut bench.herdr_stream {
                        if let Ok(Some(SessionOutput::Data(bytes))) = bench.rt.block_on(async {
                            tokio::time::timeout(Duration::from_millis(200), stream.recv_uncredited()).await
                        }) {
                            got_output = true;
                            bench.aux_bytes += bytes.len();
                            feed_benchmark_output(&bench.rt, stream, &mut bench.vt, &bytes);
                        }
                        // Drain any trailing bytes that arrived in the same redraw burst
                        let quiesce = Instant::now() + Duration::from_millis(2);
                        while Instant::now() < quiesce {
                            match bench.rt.block_on(async {
                                tokio::time::timeout(Duration::from_millis(1), stream.recv_uncredited()).await
                            }) {
                                Ok(Some(SessionOutput::Data(bytes))) => {
                                    bench.aux_bytes += bytes.len();
                                    feed_benchmark_output(&bench.rt, stream, &mut bench.vt, &bytes);
                                }
                                _ => break,
                            }
                        }
                    }
                    if got_output {
                        bench
                            .latencies_ms
                            .push(t_action.elapsed().as_secs_f64() * 1000.0);
                    }
                    false
                } else {
                    if let Some(h) = &bench.herdr_handle {
                        let _ = h.try_send_user(b":q!\r".to_vec());
                    }
                    bench.aux_metric_ms = now.duration_since(bench.started_at).as_secs_f64() * 1000.0;
                    true
                }
            }
            BenchScenario::Burst5MiB => {
                if frame_idx == 0 {
                    bench.aux_bytes = 0;
                    if let Some(h) = &bench.herdr_handle {
                        let _ = h.try_send_user(
                            b"python3 scripts/bench-app.py workload --mib 5 --seconds 0; printf '%s%s\\n' GPUI_ BURST_END\n"
                                .to_vec(),
                        );
                    }
                }
                let mut saw_end = false;
                if let Some(stream) = &mut bench.herdr_stream {
                    let mut budget = 0usize;
                    while budget < 256 * 1024 {
                        match stream.try_recv_uncredited() {
                            Some(SessionOutput::Data(bytes)) => {
                                budget += bytes.len();
                                bench.aux_bytes += bytes.len();
                                if bytes
                                    .windows(b"GPUI_BURST_END".len())
                                    .any(|w| w == b"GPUI_BURST_END")
                                {
                                    saw_end = true;
                                }
                                feed_benchmark_output(&bench.rt, stream, &mut bench.vt, &bytes);
                            }
                            _ => break,
                        }
                    }
                }
                if saw_end || now.duration_since(bench.started_at) > Duration::from_secs(20) {
                    let total_ms = now.duration_since(bench.started_at).as_secs_f64() * 1000.0;
                    bench.aux_metric_ms = (total_ms - bench.first_paint_ms.unwrap_or(0.0)).max(0.0);
                    true
                } else {
                    false
                }
            }
            BenchScenario::InputLatency => {
                if frame_idx == 0 {
                    if let Some(h) = &bench.herdr_handle {
                        let _ = h.try_send_user(b"stty raw -echo; cat\n".to_vec());
                    }
                    std::thread::sleep(Duration::from_millis(80));
                    if let Some(stream) = &mut bench.herdr_stream {
                        while let Some(SessionOutput::Data(b)) = stream.try_recv_uncredited() {
                            feed_benchmark_output(&bench.rt, stream, &mut bench.vt, &b);
                        }
                    }
                    // Run 100 synchronous round-trips (50 ASCII + 50 Japanese UTF-8) through
                    // herdr-core PTY + VtTerminal + RowShapeCache.
                    let ts = window.text_system();
                    for idx in 0..100usize {
                        let payload: &[u8] = if idx < 50 {
                            b"a"
                        } else {
                            "あ".as_bytes()
                        };
                        let t_send = Instant::now();
                        if let Some(h) = &bench.herdr_handle {
                            let _ = h.try_send_user(payload.to_vec());
                        }
                        let mut got = false;
                        let deadline = Instant::now() + Duration::from_secs(2);
                        while Instant::now() < deadline && !got {
                            if let Some(stream) = &mut bench.herdr_stream
                                && let Ok(Some(SessionOutput::Data(bytes))) = bench.rt.block_on(async {
                                    tokio::time::timeout(Duration::from_millis(50), stream.recv_uncredited())
                                        .await
                                })
                            {
                                bench.aux_bytes += bytes.len();
                                feed_benchmark_output(&bench.rt, stream, &mut bench.vt, &bytes);
                                if bytes.windows(payload.len()).any(|w| w == payload) {
                                    got = true;
                                }
                            }
                        }
                        let _ = bench.cache.prepare_frame(
                            &mut bench.vt,
                            &bench.style,
                            window.scale_factor(),
                            ts,
                        );
                        if got {
                            bench
                                .latencies_ms
                                .push(t_send.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    if let Some(h) = &bench.herdr_handle {
                        let _ = h.try_send_user(b"\x04".to_vec());
                    }
                    true
                } else {
                    true
                }
            }
        };

        bench.total_frames += 1;
        if done {
            bench.bench_finished = true;
            Self::report_bench_and_quit(bench, cx);
            return;
        }

        window.request_animation_frame();
        cx.notify();
    }

    fn report_bench_and_quit(bench: &mut BenchState, cx: &mut Context<Self>) {
        let mut sorted = bench.frame_times_ms.clone();
        if sorted.is_empty() {
            sorted.push(0.0);
        }
        sorted.sort_by(|a, b| a.total_cmp(b));
        let p50 = sorted[sorted.len() / 2];
        let p95 = sorted[(sorted.len() * 95) / 100];
        let p99 = sorted[(sorted.len() * 99) / 100];
        let max_ms = *sorted.last().unwrap_or(&0.0);
        let over_16 = sorted.iter().filter(|&&t| t > 16.67).count();
        let over_25 = sorted.iter().filter(|&&t| t > 25.0).count();

        let mut prepaint_sorted = bench.prepaint_us_samples.clone();
        prepaint_sorted.sort_unstable();
        let prepaint_p50 = prepaint_sorted.get(prepaint_sorted.len() / 2).copied().unwrap_or(0);
        let prepaint_p95 = prepaint_sorted
            .get((prepaint_sorted.len() * 95) / 100)
            .copied()
            .unwrap_or(0);

        let main_rss = current_rss_mib();
        let core_pid = bench.herdr_guard.as_ref().map(|g| g.core_pid()).unwrap_or(0);
        let core_rss = pid_rss_mib(core_pid);
        let total_lookups = bench.cache_hits_total + bench.cache_misses_total;
        let hit_rate = if total_lookups > 0 {
            (bench.cache_hits_total as f64 / total_lookups as f64) * 100.0
        } else {
            0.0
        };

        println!(
            "=== GPUI Window Benchmark [scenario={} mode={} {}x{}] ===",
            bench.scenario.label(),
            bench.cache.mode.label(),
            bench.vt.cols(),
            bench.vt.rows()
        );
        println!(
            "  first_paint_ms   : {:.2} ms",
            bench.first_paint_ms.unwrap_or(0.0)
        );
        println!(
            "  aux_ms           : {:.2} ms (bytes={:.1} KiB)",
            bench.aux_metric_ms,
            bench.aux_bytes as f64 / 1024.0
        );
        println!(
            "  frame_interval   : n={} p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms >16.67ms={} >25ms={}",
            bench.frame_times_ms.len(),
            p50,
            p95,
            p99,
            max_ms,
            over_16,
            over_25
        );
        println!(
            "  prepaint_us      : p50={}us p95={}us shape_calls={} dirty_rows={} cache_hit_rate={:.1}% ({}/{}) max_entries={} max_cache_kib={:.2}",
            prepaint_p50,
            prepaint_p95,
            bench.shape_calls_total,
            bench.dirty_rows_total,
            hit_rate,
            bench.cache_hits_total,
            total_lookups,
            bench.max_cached_entries,
            bench.max_cache_bytes as f64 / 1024.0
        );
        if !bench.latencies_ms.is_empty() {
            let mut lats = bench.latencies_ms.clone();
            lats.sort_by(|a, b| a.total_cmp(b));
            let lp50 = lats[lats.len() / 2];
            let lp95 = lats[(lats.len() * 95) / 100];
            let lp99 = lats[(lats.len() * 99) / 100];
            let lmax = *lats.last().unwrap_or(&0.0);
            println!(
                "  input_latency_ms : n={} p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms",
                lats.len(),
                lp50,
                lp95,
                lp99,
                lmax
            );
        }
        println!(
            "  rss_mib          : gpui={:.2} MiB, herdr-core={:.2} MiB, total={:.2} MiB",
            main_rss,
            core_rss,
            main_rss + core_rss
        );

        bench.herdr_stream = None;
        bench.herdr_handle = None;
        if let Some(mut guard) = bench.herdr_guard.take() {
            guard.stop_blocking();
        }
        if let Some(p) = bench.tmp_file.take() {
            let _ = std::fs::remove_file(p);
        }
        let qa_home = PathBuf::from(format!("/tmp/u-gp-nvhome-{}", std::process::id()));
        if qa_home.exists() {
            let _ = std::fs::remove_dir_all(qa_home);
        }
        cx.quit();
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

struct VtBenchScreenElement {
    view: Entity<GpuiPocView>,
}

struct VtBenchPrepaint {
    prepared: Option<PreparedTerminalFrame>,
}

impl IntoElement for VtBenchScreenElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for VtBenchScreenElement {
    type RequestLayoutState = ();
    type PrepaintState = VtBenchPrepaint;

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
        let scale_factor = window.scale_factor();
        let ts = window.text_system();
        let prepared = self.view.update(cx, |v, _| {
            let b = v.bench.as_mut()?;
            let mut cache = std::mem::replace(&mut b.cache, RowShapeCache::new(RenderCacheMode::Uncached));
            let res = cache.prepare_frame(&mut b.vt, &b.style, scale_factor, ts).ok();
            let stats = cache.last_stats;
            b.prepaint_us_samples.push(stats.prepaint_us);
            b.shape_calls_total += stats.shape_calls;
            b.dirty_rows_total += stats.dirty_rows;
            b.cache_hits_total += stats.cache_hits;
            b.cache_misses_total += stats.cache_misses;
            b.max_cached_entries = b.max_cached_entries.max(stats.cached_entries);
            b.max_cache_bytes = b.max_cache_bytes.max(stats.estimated_cache_bytes);
            b.cache = cache;
            res
        });
        VtBenchPrepaint { prepared }
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
        let Some(frame) = prepaint.prepared.as_ref() else {
            return;
        };
        let content_bounds = Bounds::new(
            point(bounds.left() + px(TERM_PAD_X), bounds.top() + px(TERM_PAD_Y)),
            size(
                (bounds.size.width - px(TERM_PAD_X * 2.0)).max(px(16.0)),
                (bounds.size.height - px(TERM_PAD_Y * 2.0)).max(px(18.0)),
            ),
        );
        let _ = paint_terminal_frame(frame, content_bounds, true, true, None, window, cx);
    }
}

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
                .child(VtBenchScreenElement {
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
    let raw_os_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if raw_os_args.get(1).is_some_and(|a| a == os::PTY_IUTF8_INIT_FLAG) {
        os::run_pty_iutf8_init(&raw_os_args[2..]);
    }

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--bench-vt") {
        vt::run_headless_vt_benchmark();
        return;
    }

    #[cfg(unix)]
    let quit_signal = install_signal_cleanup_watcher();

    let cfg = CliConfig::parse(&args);
    let started_at = Instant::now();

    Application::new().run(move |cx: &mut App| {
        #[cfg(unix)]
        if let Some(signal) = quit_signal {
            let exit_signal = std::rc::Rc::new(std::cell::Cell::new(None));
            cx.on_app_quit({
                let exit_signal = exit_signal.clone();
                move |_| {
                    let signal = exit_signal.get();
                    async move {
                        // GPUI polls shutdown futures after running the synchronous cleanup callbacks.
                        if let Some(signal) = signal {
                            let signal = libc::c_int::from(signal);
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
                if let Ok(signal) = signal.await {
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
        } else {
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
                    cx.new(|cx| LiveTerminalView::new(cfg_clone, started_at, window, cx))
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
