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
