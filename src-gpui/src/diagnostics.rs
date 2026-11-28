//! Development-only font, IME and performance probes.

use super::*;

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
) -> Result<(), &'static str> {
    let replies = vt.feed(bytes).pty_writes;
    rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(2),
            stream.acknowledge(bytes.len() as u32, replies),
        )
        .await
    })
    .map_err(|_| "benchmark writer capacity timed out")?
    .map_err(|_| "benchmark writer closed")
}

const BURST_WORKLOAD_COMMAND: &[u8] = b"if python3 scripts/bench-app.py workload --mib 5 --seconds 0; then printf '%s%s\\n' GPUI_BURST_ OK; else printf '%s%s\\n' GPUI_BURST_ FAILED; fi\n";

fn contains_stream_marker(tail: &mut Vec<u8>, bytes: &[u8], marker: &[u8]) -> bool {
    let limit = marker.len() - 1;
    let prefix = bytes.len().min(limit);
    tail.extend_from_slice(&bytes[..prefix]);
    let found = tail.windows(marker.len()).any(|part| part == marker)
        || bytes.windows(marker.len()).any(|part| part == marker);

    if bytes.len() >= limit {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - limit..]);
    } else if tail.len() > limit {
        tail.drain(..tail.len() - limit);
    }
    found
}

struct BenchState {
    scenario: BenchScenario,
    started_at: Instant,
    first_render_ms: Option<f64>,
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
    scratch: Option<DirCleanupGuard>,
    marker_tail: Vec<u8>,
    failure_tail: Vec<u8>,
    failure: Option<&'static str>,
    latency_ready: bool,
}

pub(super) struct GpuiPocView {
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
    pub(super) fn new(
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
            if let Some(bench) = &mut this.bench {
                if let Some(mut guard) = bench.herdr_guard.take() {
                    guard.stop_blocking();
                }
                bench.scratch = None;
            }
            std::future::ready(())
        })
        .detach();

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
        let mut vt = VtTerminal::new(cfg.cols, cfg.rows, cfg.scrollback_lines).expect("vt new");
        let cache = RowShapeCache::new(cfg.render_mode);
        let mut aux_metric_ms = 0.0;
        let mut aux_bytes = 0usize;
        let mut failure = None;
        let mut scratch = None;

        let (herdr_guard, herdr_handle, mut herdr_stream) = match cfg.scenario {
            BenchScenario::NvimCtrlDu => {
                let dir = DirCleanupGuard::create("nvim-bench")
                    .expect("create Neovim benchmark directory");
                let sample_path = dir.path().join("sample.rs");
                let mut content = String::with_capacity(64 * 1024);
                for i in 0..1000 {
                    content.push_str(&format!(
                        "// line {i:04}: 日本語コメントと \u{E0A0} main シンボル解析テスト\nfn sample_fn_{i:04}(arg: usize) -> usize {{ arg.saturating_add({i}) }}\n"
                    ));
                }
                std::fs::write(&sample_path, content).expect("write nvim sample");
                let mut env = os::build_child_shell_env(Some("/bin/sh"));
                let qa_home = dir.path().to_string_lossy();
                env.insert("HOME".into(), qa_home.to_string());
                env.insert("XDG_CONFIG_HOME".into(), format!("{qa_home}/cfg"));
                env.insert("XDG_STATE_HOME".into(), format!("{qa_home}/st"));
                env.insert("XDG_DATA_HOME".into(), format!("{qa_home}/data"));
                env.insert("XDG_CACHE_HOME".into(), format!("{qa_home}/cache"));
                scratch = Some(dir);

                let opts = HerdrStartOptions {
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
                let marker = b"sample_fn_0000";
                let mut tail = Vec::new();
                let mut ready = false;
                while Instant::now() < deadline {
                    match rt.block_on(async {
                        tokio::time::timeout_at(deadline.into(), stream.recv_uncredited()).await
                    }) {
                        Ok(Some(SessionOutput::Data(bytes))) => {
                            aux_bytes += bytes.len();
                            if let Err(error) = feed_benchmark_output(&rt, &stream, &mut vt, &bytes)
                            {
                                failure = Some(error);
                                break;
                            }
                            if contains_stream_marker(&mut tail, &bytes, marker) {
                                ready = true;
                                break;
                            }
                        }
                        Ok(Some(SessionOutput::Resize { .. })) => continue,
                        Ok(Some(SessionOutput::Closed(_)) | None) | Err(_) => break,
                    }
                }
                if !ready {
                    failure.get_or_insert("Neovim did not reach the benchmark sample");
                }
                (Some(guard), Some(handle), Some(stream))
            }
            BenchScenario::Burst5MiB | BenchScenario::InputLatency => {
                let opts = HerdrStartOptions {
                    argv: Some(vec!["/bin/sh".into()]),
                    cwd: (cfg.scenario == BenchScenario::Burst5MiB).then(|| {
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .parent()
                            .expect("repository root")
                            .to_string_lossy()
                            .into_owned()
                    }),
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
                if handle
                    .try_send_user(
                        b"seq 1 10000 && printf '%s%s\\n' UITERM_GPUI_ BENCH_DONE\n".to_vec(),
                    )
                    .is_err()
                {
                    failure = Some("could not start scroll-stream workload");
                }
                let mut saw_done = false;
                let mut tail = Vec::new();
                let deadline = Instant::now() + Duration::from_secs(10);
                while Instant::now() < deadline && !saw_done && failure.is_none() {
                    match rt.block_on(async {
                        tokio::time::timeout(Duration::from_millis(200), stream.recv_uncredited())
                            .await
                    }) {
                        Ok(Some(SessionOutput::Data(bytes))) => {
                            aux_bytes += bytes.len();
                            saw_done = contains_stream_marker(
                                &mut tail,
                                &bytes,
                                b"UITERM_GPUI_BENCH_DONE",
                            );
                            failure = feed_benchmark_output(&rt, &stream, &mut vt, &bytes).err();
                        }
                        Ok(None | Some(SessionOutput::Closed(_))) => {
                            failure = Some("scroll-stream connection closed");
                        }
                        Ok(Some(SessionOutput::Resize { .. })) | Err(_) => {}
                    }
                }
                if !saw_done {
                    failure.get_or_insert("scroll-stream workload timed out");
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
            first_render_ms: None,
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
            scratch,
            marker_tail: Vec::new(),
            failure_tail: Vec::new(),
            failure,
            latency_ready: false,
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
        if bench.first_render_ms.is_none() {
            bench.first_render_ms =
                Some(now.duration_since(bench.started_at).as_secs_f64() * 1000.0);
        }
        if let Some(prev) = bench.last_frame_at {
            bench
                .frame_times_ms
                .push(now.duration_since(prev).as_secs_f64() * 1000.0);
        }
        bench.last_frame_at = Some(now);

        let frame_idx = bench.total_frames;
        let done = bench.failure.is_some()
            || match bench.scenario {
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
                    let line = format!(
                        "\r[status] step={frame_idx:05} spinner={} elapsed={}ms",
                        frame_idx % 8,
                        frame_idx * 16
                    );
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
                        let dir = if (frame_idx / 120) % 2 == 0 {
                            -step
                        } else {
                            step
                        };
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
                        if bench
                            .herdr_handle
                            .as_ref()
                            .is_none_or(|handle| handle.try_send_user(byte.to_vec()).is_err())
                        {
                            bench.failure = Some("could not send Neovim scroll input");
                        }
                        let mut got_output = false;
                        if let Some(stream) = &mut bench.herdr_stream {
                            if let Ok(Some(SessionOutput::Data(bytes))) = bench.rt.block_on(async {
                                tokio::time::timeout(
                                    Duration::from_millis(200),
                                    stream.recv_uncredited(),
                                )
                                .await
                            }) {
                                got_output = true;
                                bench.aux_bytes += bytes.len();
                                bench.failure =
                                    feed_benchmark_output(&bench.rt, stream, &mut bench.vt, &bytes)
                                        .err();
                            }
                            // Drain any trailing bytes that arrived in the same redraw burst
                            let quiesce = Instant::now() + Duration::from_millis(2);
                            while Instant::now() < quiesce && bench.failure.is_none() {
                                match bench.rt.block_on(async {
                                    tokio::time::timeout(
                                        Duration::from_millis(1),
                                        stream.recv_uncredited(),
                                    )
                                    .await
                                }) {
                                    Ok(Some(SessionOutput::Data(bytes))) => {
                                        bench.aux_bytes += bytes.len();
                                        bench.failure = feed_benchmark_output(
                                            &bench.rt,
                                            stream,
                                            &mut bench.vt,
                                            &bytes,
                                        )
                                        .err();
                                    }
                                    Ok(Some(SessionOutput::Closed(_)) | None) => {
                                        bench.failure = Some("Neovim connection closed");
                                    }
                                    _ => break,
                                }
                            }
                        }
                        if !got_output {
                            bench
                                .failure
                                .get_or_insert("Neovim scroll produced no output");
                        } else if bench.failure.is_none() {
                            bench
                                .latencies_ms
                                .push(t_action.elapsed().as_secs_f64() * 1000.0);
                        }
                        false
                    } else {
                        if let Some(h) = &bench.herdr_handle {
                            let _ = h.try_send_user(b":q!\r".to_vec());
                        }
                        bench.aux_metric_ms =
                            now.duration_since(bench.started_at).as_secs_f64() * 1000.0;
                        true
                    }
                }
                BenchScenario::Burst5MiB => {
                    if frame_idx == 0
                        && bench.herdr_handle.as_ref().is_none_or(|handle| {
                            handle
                                .try_send_user(BURST_WORKLOAD_COMMAND.to_vec())
                                .is_err()
                        })
                    {
                        bench.failure = Some("could not start burst workload");
                    }
                    let mut saw_end = false;
                    if let Some(stream) = &mut bench.herdr_stream {
                        let mut budget = 0;
                        while budget < 256 * 1024 && !saw_end && bench.failure.is_none() {
                            match stream.try_recv_uncredited() {
                                Some(SessionOutput::Data(bytes)) => {
                                    budget += bytes.len();
                                    bench.aux_bytes += bytes.len();
                                    saw_end = contains_stream_marker(
                                        &mut bench.marker_tail,
                                        &bytes,
                                        b"GPUI_BURST_OK",
                                    );
                                    if contains_stream_marker(
                                        &mut bench.failure_tail,
                                        &bytes,
                                        b"GPUI_BURST_FAILED",
                                    ) {
                                        bench.failure =
                                            Some("burst workload exited unsuccessfully");
                                    }
                                    if let Err(error) = feed_benchmark_output(
                                        &bench.rt,
                                        stream,
                                        &mut bench.vt,
                                        &bytes,
                                    ) {
                                        bench.failure.get_or_insert(error);
                                    }
                                }
                                Some(SessionOutput::Closed(_)) => {
                                    bench.failure = Some("burst workload connection closed");
                                }
                                Some(SessionOutput::Resize { .. }) => continue,
                                None => break,
                            }
                        }
                    }
                    let elapsed_ms = bench.started_at.elapsed().as_secs_f64() * 1000.0
                        - bench.first_render_ms.unwrap_or(0.0);
                    if saw_end {
                        bench.aux_metric_ms = elapsed_ms;
                    } else if elapsed_ms > 20_000.0 {
                        bench.failure.get_or_insert("burst workload timed out");
                    }
                    saw_end || bench.failure.is_some()
                }
                BenchScenario::InputLatency => Self::step_input_latency(bench, window, frame_idx),
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

    fn step_input_latency(bench: &mut BenchState, window: &Window, frame_idx: usize) -> bool {
        let (Some(handle), Some(stream)) = (&bench.herdr_handle, &mut bench.herdr_stream) else {
            bench.failure = Some("input-latency attachment is unavailable");
            return true;
        };
        if frame_idx == 0 {
            if handle
                .try_send_user(
                    b"stty raw -echo && printf '%s%s' GPUI_LATENCY_ READY && exec cat\n".to_vec(),
                )
                .is_err()
            {
                bench.failure = Some("could not start input-latency workload");
                return true;
            }
            return false;
        }

        let (payload, sent_at) = if bench.latency_ready {
            let payload: &[u8] = if bench.latencies_ms.len() < 50 {
                b"a"
            } else {
                "あ".as_bytes()
            };
            let sent_at = Instant::now();
            if handle.try_send_user(payload.to_vec()).is_err() {
                bench.failure = Some("input-latency input could not be sent");
                return true;
            }
            (payload, sent_at)
        } else {
            (&b"GPUI_LATENCY_READY"[..], Instant::now())
        };

        let event = bench.rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(30), async {
                while let Some(event) = stream.recv_uncredited().await {
                    match event {
                        SessionOutput::Data(bytes) => {
                            bench.aux_bytes += bytes.len();
                            let matched =
                                contains_stream_marker(&mut bench.marker_tail, &bytes, payload);
                            stream
                                .acknowledge(bytes.len() as u32, bench.vt.feed(&bytes).pty_writes)
                                .await?;
                            if matched {
                                return Ok(true);
                            }
                        }
                        SessionOutput::Closed(_) => break,
                        SessionOutput::Resize { .. } => {}
                    }
                }
                Ok::<_, String>(false)
            })
            .await
        });
        match event {
            Ok(Ok(true)) if !bench.latency_ready => {
                bench.latency_ready = true;
                bench.marker_tail.clear();
                false
            }
            Ok(Ok(true)) => {
                let _ = bench.cache.prepare_frame(
                    &mut bench.vt,
                    &bench.style,
                    window.scale_factor(),
                    window.text_system(),
                );
                bench
                    .latencies_ms
                    .push(sent_at.elapsed().as_secs_f64() * 1000.0);
                bench.marker_tail.clear();
                bench.latencies_ms.len() == 100
            }
            Err(_)
                if !bench.latency_ready && bench.started_at.elapsed() < Duration::from_secs(2) =>
            {
                false
            }
            Err(_) => {
                bench.failure = Some("input-latency wait timed out");
                true
            }
            Ok(_) => {
                bench.failure = Some("input-latency connection closed");
                true
            }
        }
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
        let prepaint_p50 = prepaint_sorted
            .get(prepaint_sorted.len() / 2)
            .copied()
            .unwrap_or(0);
        let prepaint_p95 = prepaint_sorted
            .get((prepaint_sorted.len() * 95) / 100)
            .copied()
            .unwrap_or(0);

        let main_rss = current_rss_mib();
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
            "  first_render_ms   : {:.2} ms",
            bench.first_render_ms.unwrap_or(0.0)
        );
        if let Some(failure) = bench.failure {
            eprintln!("  status           : failed ({failure})");
        } else {
            println!(
                "  aux_ms           : {:.2} ms (bytes={:.1} KiB)",
                bench.aux_metric_ms,
                bench.aux_bytes as f64 / 1024.0,
            );
        }
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
        if bench.failure.is_none() && !bench.latencies_ms.is_empty() {
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
        let rss_label = |rss: Option<f64>| {
            rss.map_or_else(
                || "unavailable".to_string(),
                |value| format!("{value:.2} MiB"),
            )
        };
        println!("  rss_mib          : gpui={}", rss_label(main_rss));

        bench.herdr_stream = None;
        bench.herdr_handle = None;
        if let Some(mut guard) = bench.herdr_guard.take() {
            guard.stop_blocking();
        }
        bench.scratch = None;
        if bench.failure.is_some() {
            std::process::exit(1);
        }
        cx.quit();
    }

    fn run_font_probe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ts = window.text_system();
        let all_fonts = ts.all_font_names();
        println!("=== GPUI Font & Glyph Shaping Probe ===");
        println!(
            "Total system font families visible to GPUI: {}",
            all_fonts.len()
        );
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
            println!(
                "  family {:<24}: {}",
                fam,
                if found { "FOUND" } else { "missing" }
            );
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
            point(
                bounds.left() + end_x.max(start_x + px(2.0)),
                bounds.top() + row_h,
            ),
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
        let local_x = point.x - bounds.left();
        if local_x < px(0.0)
            || local_x >= line.width
            || point.y < bounds.top()
            || point.y >= bounds.bottom()
        {
            return None;
        }
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
            let res = b
                .cache
                .prepare_frame(&mut b.vt, &b.style, scale_factor, ts)
                .ok();
            let stats = b.cache.last_stats;
            b.prepaint_us_samples.push(stats.prepaint_us);
            b.shape_calls_total += stats.shape_calls;
            b.dirty_rows_total += stats.dirty_rows;
            b.cache_hits_total += stats.cache_hits;
            b.cache_misses_total += stats.cache_misses;
            b.max_cached_entries = b.max_cached_entries.max(stats.cached_entries);
            if b.cache.mode == RenderCacheMode::DirtyRowCache {
                b.max_cache_bytes = b.max_cache_bytes.max(b.cache.estimated_bytes());
            }
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
            point(
                bounds.left() + px(TERM_PAD_X),
                bounds.top() + px(TERM_PAD_Y),
            ),
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
                let shaped =
                    ts.shape_line(cell.grapheme.to_string().into(), font_size, &[run], None);
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
                let shaped =
                    ts.shape_line(cell.grapheme.to_string().into(), font_size, &[run], None);
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
                Some((
                    ime_line.x_for_index(mark.start),
                    ime_line.x_for_index(mark.end),
                ))
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
            point(
                bounds.left() + px(16.0),
                bounds.top() + prepaint.ime_origin_y_offset,
            ),
            size(
                (bounds.size.width - px(32.0)).max(px(120.0)),
                prepaint.row_height + px(12.0),
            ),
        );
        let ime_text_origin = point(
            ime_box_bounds.left() + px(8.0),
            ime_box_bounds.top() + px(6.0),
        );
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
            let shaped_label =
                window
                    .text_system()
                    .shape_line(label.clone().into(), px(12.0), &[label_run], None);
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
                    .child(div().ml_auto().text_color(rgb(FG_MUTED)).child(format!(
                        "{:.1}px | cell {:.2}x{:.0}px",
                        self.font_size, cell_w, row_h
                    ))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_command_marks_workload_success_and_failure_separately() {
        let command = std::str::from_utf8(BURST_WORKLOAD_COMMAND).unwrap();
        for (code, marker) in [(0, "GPUI_BURST_OK"), (7, "GPUI_BURST_FAILED")] {
            let script = format!("python3() {{ return {code}; }}\n{command}");
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), marker);
        }
    }

    #[test]
    fn stream_marker_is_found_across_chunks_without_retaining_the_output() {
        let marker = b"ready";
        let mut tail = Vec::new();
        assert!(!contains_stream_marker(
            &mut tail,
            b"long output re",
            marker
        ));
        assert!(!contains_stream_marker(&mut tail, b"a", marker));
        assert!(contains_stream_marker(
            &mut tail,
            b"dy trailing output",
            marker
        ));
        assert_eq!(tail, b"tput");
    }
}
