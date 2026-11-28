# Phase 2 GPUI製品移行

基準は移行前のmain `0d34b0a`。Tauri版は比較基準として引き続き起動できる。Phase 2は未完了。

| パス                                                      | 責務                                                                      |
| --------------------------------------------------------- | ------------------------------------------------------------------------- |
| `src-gpui/src/main.rs`                                    | 製品バイナリ `uiterm` の起動、端末ウィンドウ、入力とPTY出力の取り込み     |
| `src-gpui/src/herdr.rs`                                   | セッションとアタッチの分離、Framed Mode、出力クレジット、自動応答         |
| `src-gpui/src/{renderer,vt,fonts,ime,input,os,locale}.rs` | GPUI描画、ネイティブVT、文字幅、未確定入力、キーバインド、IUTF8、日英表示 |
| `src-gpui/src/diagnostics.rs`                             | フォント・IMEプローブとウィンドウベンチマーク                             |
| `native/herdr`                                            | GPUI・Tauri共通のcore起動、監視、復旧、終了                               |
| `src-gpui/tests/compatibility.rs`                         | 実core・PTYの再接続、IUTF8と隔離CLIの互換テスト                           |

監視プロセスが専用ディレクトリとcoreを所有する。GUIが終了するとstdinのEOFを受けて自分のcoreを終了する。coreの異常終了後にPTYが生存している場合、監視プロセスがcoreを復旧し、ユーザーが「再接続」を押すと同じセッションに新しいアタッチを作る。VTを初期化し、coreが再送する履歴と画面を適用する。PTY終了後の「再起動」は新しいセッションを作る。

GPUIの描画、IME、Nerd Font、Unicode幅、スクロール、選択、クリップボード、Alternate Screen、同期出力はPoCから引き継いだ。共通Action API、内部設定、TypeScript設定・拡張、タブや分割は別工程。現在の端末UIは呼び出し箇所に日英の文言を持ち、OSロケールで選択する。翻訳キーは共通Action APIの移植と合わせて扱う。coreの標準エラー出力は、端末データを診断ログに残さないため両版で破棄する。

GPUI 0.2.2の配布クレートはTaffyを `=0.9.0` と指定しており、`src-gpui/Cargo.lock` でもTaffy 0.9.0が解決されている。このTaffyは `grid 0.18.0` を要求するため、修正版 `grid 1.0.1` への互換更新はできない。依存の固定を維持し、CLI・ウィンドウ計算・通信・VTの寸法を各軸1〜1,000に制限した。端末セルはGPUIのCSS Gridに渡さず、測定済みセル幅に基づいて描画する。[GPUIパッケージ索引](https://index.crates.io/gp/ui/gpui) · [Taffy 0.9.0依存定義](https://github.com/DioxusLabs/taffy/blob/v0.9.0/Cargo.toml)

## 検証と性能

比較する場合は移行前と移行後のリリースバイナリを用い、Menlo 14px・106×32・行キャッシュ有効・履歴2,000行を指定する。診断は同じ引数で `--bench-window --scenario` を実行する。ライブウィンドウのプロセス計測は `scripts/bench-app.py measure --binary` で行い、`--core-socket auto` で専用coreとholderを含める。Tauri版のバンドルは従来どおり `--app` を使う。

### 自動検証

macOS 15.5 ARM64で次を確認した。

- GPUIはRustfmt、Clippy（`--all-targets -- -D warnings`）、デバッグ・リリースビルドに合格。バイナリのテスト38件、統合テストバイナリ20件、隔離CLIテスト1件が通過した。テスト件数には統合テストが取り込むモジュールの重複実行を含む。
- 実herdr-coreのテストでは通常の再アタッチとcore異常終了後の再アタッチで、同じPTYの画面と実行中コマンドに戻れることを確認した。IUTF8による日本語Backspace、リサイズ、Alternate Screen、fish、Neovim、`codex --version` も隔離PTYで確認した。
- TauriはRustfmt、Clippy、デバッグビルド、Rustテスト25件と隔離CLIテスト1件に合格した。TypeScriptは `pnpm check`、`lint`、`typecheck`、`test`（56件）、`build` が通過した。
- Flakeの `aarch64-darwin`・`aarch64-linux`・`x86_64-linux` の評価、計測スクリプトのセルフテスト、`git diff --check` が通過した。GitHub Actions上の実行とLinuxネイティブビルドは未確認。GPUIの既存依存 `block` と `proc-macro-error2` にはCargoの将来互換性通知が残る。

### 性能比較

Apple M3、macOS 15.5、固定版herdr-core v0.1.22で計測した。ウィンドウ内ベンチマークは、成功した試行の指標の中央値を示す。フレーム間隔には診断用の同期PTY読み取りと出力クレジット返却の待ち時間を含む。Neovimの操作RTTは入力送信からPTY出力処理までの近似値で、隣接する操作の出力を厳密には区別しない。

| 指標                                        |   移行前PoC |  GPUI製品版 |
| ------------------------------------------- | ----------: | ----------: |
| Neovim Ctrl+D/U 60往復の総計                | 2,690.71 ms | 2,466.88 ms |
| 同操作のRTT p95                             |     4.84 ms |     5.09 ms |
| 5 MiBバースト出力の完了                     |   480.57 ms |   435.08 ms |
| 5 MiBバースト中のフレーム間隔 p95           |    45.81 ms |    40.53 ms |
| 360フレームのスクロール中のフレーム間隔 p95 |    17.61 ms |    17.61 ms |

入力遅延は各100入力で、入力送信からPTY応答・VT処理・描画準備までを計測した。初回は並行ビルド中に製品版1試行がタイムアウトした。初回の成功試行のp95中央値はPoC 7.25 ms（3/3試行）、製品版7.69 ms（2/3試行）。並行ビルド終了後の再測定は両版3/3試行が完走し、p95中央値はPoC 1.43 ms、製品版1.00 msだった。タイムアウトと並行ビルドの因果関係は未確認。

ライブ端末では、実測106×32、256 KiB/sを30秒流し、専用の監視プロセス・core・holder・子プロセスを含めて約1秒間隔で各30点を取得した。各版1試行で、CPUは `ps` の `%CPU` サンプル平均。

| 指標                             |           移行前PoC |          GPUI製品版 |
| -------------------------------- | ------------------: | ------------------: |
| Physical Footprint 平均 / ピーク | 140.60 / 147.16 MiB | 143.02 / 150.77 MiB |
| RSS 平均 / ピーク                | 119.66 / 138.38 MiB | 119.76 / 128.22 MiB |
| CPU 平均                         |               6.85% |               6.53% |
| 実行ファイル                     |         6,354,256 B |         6,404,816 B |

両版ともライブウィンドウの表示、ワークロード完走、終了後の専用ソケットの消滅を確認した。試行数とCPUの取得方式から、小さな差の再現性は判断できない。

### 残る実機検証

macOSでは日本語IMEの未確定とフォーカス復帰、候補位置、Nerd Font・絵文字の見え方、選択・クリップボード、スクロール・ドラッグリサイズ、対話Neovim・Codexを製品版の実画面で確認する。手順は [実機検証](../poc/gpui-terminal.md#8-実機検証手順と起動コマンド) を参照。NixOS / Hyprland実機、可視アイドルCPUの追加最適化、配布パッケージは後続工程。

## 安定性修正（Issues #10〜#13）

`main`（`3f5a87f`）から分岐したローカルブランチ `fix/gpui-terminal-stability` で、GPUI製品版に対する4件の安定性修正を実施した（未マージ・Phase 2は未完了のまま維持）。

### 修正概要

- **#10 大量出力時の書き込み飢餓**: `src-gpui/src/herdr.rs` の `spawn_framed_connection` から `tokio::select!` の `biased;` を除去し、連続出力中でもユーザー入力・`Ctrl+C`・リサイズ・クレジット・VT自動応答の書き込みが公平に進行するようにした。フレーム順序、チャネル上限、切断検出は維持した。
- **#11 IME処理**: `src-gpui/src/ime.rs` で UTF-16 ↔ UTF-8 範囲変換を逆転範囲正規化・文字境界クランプ付きの `range_from_utf16_in` に統一し、`TerminalImeState::replace_text_in_range` の部分確定時に後続未確定文字列を `preedit` / `marked_range` に残すようにした。`src-gpui/src/main.rs` と `src-gpui/src/diagnostics.rs` の `bounds_for_range` では古い `ShapedLine` 参照を防ぎ、未確定中の `Escape` と `platform` ショートカットを `PassToIme` へ委譲して OS 側 IME との同期を保った。
- **#13 シグナル安全・環境互換性**: `src-gpui/src/main.rs` の独自シグナルパイプ（`static mut`・`errno` 非退避・`EINTR` 未処理）を `tokio::signal::unix::signal` 監視へ置き換えた。`create_private_dir_in` で `AlreadyExists` を回避し、Linux ではズームを `Ctrl+Shift+Up/Down/Home` に変更して `Ctrl+-`（`0x1f`）の横取りを防いだ。さらに US ANSI Shift ペアのハードコードを排してレイアウト記号のコードポイントを使うようにし、Linux のモノスペースフォールバックと `/proc/self/statm` による RSS 取得を追加した。
- **#12 描画・マウス操作**: `src-gpui/src/main.rs` でマウス押下・ホイールリスナーを端末本体領域へ限定し、ツールバー／セッションバーボタンへ `cx.stop_propagation()`、ホバー移動へ領域判定を追加した。`src-gpui/src/vt.rs` では `selection_press` / `selection_drag` の X 座標を丸めセル幅へスケーリングして小数セル幅のズレを解消し、`cell_pixel_size` 変化時はグリッド寸法が同じでも `vt.resize` とピクセル報告を更新するようにした。ワイド文字先頭セルでもカーソル幅と IME アンカーを 2 セル幅で揃え、`src-gpui/src/renderer.rs` に SGR 53（overline）描画を追加した。

### 自動検証結果（`fix/gpui-terminal-stability`）

macOS 15.5 ARM64（`herdr-core` v0.1.22）で以下が合格した。

- `src-gpui`: `cargo fmt --check`、Clippy（`--all-targets -- -D warnings`）、デバッグ・リリースビルド、単体テスト 61 件、結合テスト 30 件（+ 隔離 CLI 1 件通過）。なお、`pty_iutf8_init_enables_utf8_backspace_on_real_herdr_pty` は並列実行時の負荷により1度タイムアウト（5.52秒）した後に再実行・全検証一括実行・単独実行のいずれも通過した。
- `src-tauri`: `cargo fmt --check`、Clippy（`--all-targets -- -D warnings`）、デバッグ・リリースビルド、単体テスト 16 件、結合テスト 6 件、監視テスト 3 件、隔離 CLI 1 件が通過。
- TypeScript / Nix / Git: `pnpm check`、`pnpm lint`、`pnpm typecheck`、`pnpm test`（11 ファイル 56 件）、`pnpm build`、`nix flake check --no-build --all-systems`、`git diff --check` が通過。ロックファイルは変更していない。
- Linux ネイティブビルド、NixOS / Hyprland（Wayland + Fcitx5）、および実機 GUI 操作は未検証。

### 性能比較（`main` `3f5a87f` vs `fix/gpui-terminal-stability`）

Apple M3、macOS 15.5、リリースビルド、Menlo 14px・`106x32`・行キャッシュ有効・履歴 2,000 行、並列ビルドなしの同一条件で各 3 試行（交互実行）を計測した。値は 3 試行の中央値（ライブ端末 CPU は `ps` の累積 CPU 時間差分から算出した 15 サンプル平均の各試行値の中央値）。

| 指標                                          |           `main` (`3f5a87f`) | `fix/gpui-terminal-stability` |
| --------------------------------------------- | ---------------------------: | ----------------------------: |
| ライブ アイドル CPU 平均 / RSS 平均 / ピーク  |    0.81% / 68.31 / 75.41 MiB |     1.06% / 70.06 / 76.42 MiB |
| ライブ 256 KiB/s CPU 平均 / RSS 平均 / ピーク |  5.34% / 119.03 / 125.42 MiB |   4.99% / 122.90 / 130.66 MiB |
| 入力遅延 p50 / p95（100入力）                 |               0.40 / 3.67 ms |                0.43 / 4.57 ms |
| スクロール（360フレーム）間隔 p95             |                     17.65 ms |                      17.68 ms |
| Neovim Ctrl+D/U 60往復総計 / RTT p50 / p95    | 2,459.64 ms / 2.70 / 5.27 ms |  2,474.88 ms / 3.64 / 5.05 ms |
| 5 MiB バースト完了 / フレーム間隔 p95         |            567.81 / 31.48 ms |             589.94 / 40.09 ms |
| 実行ファイルサイズ                            |                  6,428,944 B |                   6,429,264 B |

ライブ端末全 12 試行（両版各 6 試行）で SIGTERM 終了後の隔離 `herdr-core` ソケット残存は 0 件だった。`input-latency` p95 や `burst-5mib` フレーム間隔 p95（`n=22..39` フレームのため 1〜2 フレームの外れ値に左右される）の差は試行間の範囲が重なっており、3 試行の範囲で再現性のある退行は確認されなかった。

### 追加の実機確認項目

[実機検証手順](../poc/gpui-terminal.md#8-実機検証手順と起動コマンド) に加えて、修正後は以下を実機で確認する。

- 連続大量出力（`yes` 等）の実行中に `Ctrl+C` 停止、キー入力、ウィンドウリサイズが遅延なく効くこと。
- 日本語 IME・macSKK で複数文節の連続部分確定、変換中 `Esc` キャンセル、`Cmd+Tab` 往復後の状態維持、ワイド文字上の候補ウィンドウ追従を確認すること。
- Neovim 等のマウストラッキング有効中にツールバーボタン操作で `(0, 0)` 誤クリックや選択破壊が起きないこと、小数セル幅・ズーム変更時の選択境界一致、ワイド文字カーソル幅・SGR 53（overline）表示を確認すること。
- Linux（NixOS / Hyprland）・JIS 配列環境で `Ctrl+-`（Readline Undo 等）、`Ctrl+Shift+Up/Down/Home`、`Ctrl+@` / `Ctrl+:`、Linux フォールバックフォント、Fcitx5 変換を確認すること。
