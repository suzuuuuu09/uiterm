---
description: "uiterm Phase 2（ターミナル）実装記録・性能測定・IUTF8調査・手動検証手順・残存課題"
---

# Phase 2 ターミナル 実装レポート

本ドキュメントは、uiterm Phase 2（ターミナル）における製品向けターミナル機能の整備、フォント・Nerd Font 対応、日本語 IME および `herdr-core` PTY `IUTF8` 調査、キーボード操作・Action Registry 接続、通常利用向け UI と PoC 診断 UI の分離、性能測定結果、手動検証チェックリスト、および残存課題をまとめたものである。

---

## 1. 実装内容

Phase 0・Phase 1 で構築した Tauri 2 + `herdr-core` v0.1.22 + `@wterm/dom` + `@wterm/ghostty`（libghostty WASM）+ Zustand Vanilla + Action Registry 基盤を維持し、不要な再実装や依存追加を行わず不足機能を補完した。

### 1.1 ターミナル基本機能

| 機能 | 実装ファイル | 内容 |
| --- | --- | --- |
| シェル起動・終了・再起動 | `src/App.tsx`, `src/actions/builtin.ts`, `src-tauri/src/commands.rs` | 既存のログインシェル起動・XDG 復元・監視プロセス終了を維持。アクティブセッションが `finished` / `lost` になった際に終了コードまたは切断状態と再起動ボタンを表示。再起動中の重複呼び出しを拒否し、失敗時も再試行できる。 |
| テキスト選択・コピー・ペースト | `src/terminal/wtermAdapter.ts`, `src/terminal/sessionManager.ts`, `src/actions/builtin.ts` | `terminal.copySelection` に加え、`terminal.paste` / `terminal.selectAll` / `terminal.clearSelection` を Action Registry へ接続。`adapter.paste(text)` は `core.bracketedPaste()` が有効な場合に ESC (`\x1b`) をサニタイズした上で `\x1b[200~...\x1b[201~` でラップして送信。 |
| リサイズ・セル計測 | `src/terminal/wtermAdapter.ts`, `src/index.css` | `ResizeObserver` による自動リサイズに加え、`document.fonts.ready` 完了時の再フィットおよびフォント変更時の即時 `fit()` を実装。バックエンド起点リサイズループ防止ガードは Phase 0/1 のまま維持。 |
| フォーカス管理 | `src/App.tsx`, `src/terminal/sessionManager.ts` | タブ切替時の `terminalFocus` IPC に加え、OS ウィンドウの `focus` / `blur` イベントを `sessionManager.setWindowFocused()` へ接続し、herdr-core の FOCUS フレームに PTY 通知を統一。タブ切替とウィンドウのアクティブ状態から通知し、wterm による同一通知の送信を抑止。 |
| カラーパレット・前景色/背景色 | `src/terminal/appearance.ts`, `src/terminal/wtermAdapter.ts`, `src/index.css` | `#09090b` 背景・`#f4f4f5` 前景・通常色（0–7）と明色（8–15）を明確に区別した 16 色 ANSI パレット（`DEFAULT_TERMINAL_THEME`）を定義。CSS カスタムプロパティ（`--term-fg`, `--term-bg`, `--term-cursor`, `--term-selection-bg`, `--term-color-0..15`）と `WTerm.setThemeColors()` を同期。 |
| カーソル形状・点滅 | `src/terminal/appearance.ts`, `src/actions/builtin.ts`, `src/index.css` | `cursorStyle`（`auto | block | bar | underline`）と `cursorBlink`（`auto | on | off`）の内部設定と `app.setCursor` アクションを追加。`auto` では VT が報告する形状を使う。明示的な形状指定は DOM / WebGL が読むカーソルに反映する。 |

### 1.2 フォント・Nerd Font

- **実装**: `src/terminal/fonts.ts`, `src/terminal/appearance.ts`, `src/validation/settingsSchemas.ts`, `src/actions/builtin.ts`
- **方針**: 特定のフォントを強制・同梱せず、システムにインストール済みのフォントを利用する。完成した設定画面と TypeScript 設定連携は Phase 4 に残し、内部 API（`buildTerminalFontStack`, `app.setFont`, `terminal.zoomIn/Out/resetFontSize`）を整備した。
- **フォールバック構成**:
  1. プライマリ等幅フォント（既定: `ui-monospace, "SF Mono", "Menlo", "JetBrains Mono"`、ユーザー設定優先）
  2. Nerd Font / PUA シンボルフォールバック（`nerdFontFallback: true` 時）: `"Symbols Nerd Font Mono"`, `"Symbols Nerd Font"`, `"UDEV Gothic 35NF"`, `"UDEV Gothic NF"`, `"JetBrainsMono Nerd Font Mono"`, `"Hack Nerd Font Mono"`, `"MesloLGS NF"`, `"FiraCode Nerd Font Mono"`, `"CaskaydiaCove Nerd Font Mono"`
  3. 日本語フォールバック: `"Hiragino Sans"`, `"Hiragino Kaku Gothic ProN"`, `"Noto Sans Mono CJK JP"`, `"BIZ UDGothic"`
  4. 絵文字フォールバック: `"Apple Color Emoji"`, `"Noto Color Emoji"`
  5. `monospace`
- **セル幅・PUA・Unicode**:
  - `GhosttyCore` WASM に対する回帰テスト（`src/terminal/__tests__/unicode.test.mjs`）に BMP PUA（`\uE0B0`, `\uE718`, `\uF015`）および Supplementary PUA-A（`\u{F024B}`）を追加し、ASCII・全角・半角カナ・曖昧幅・結合文字・ZWJ 絵文字と合わせてセル位置・カラム進行が正確であることを確認した。
  - `@wterm/dom` の `.term-row > span` は `overflow: visible` を保持しており、1 セル幅をわずかに超える Nerd Font アイコンも後続セルの位置を崩さず描画できる。

### 1.3 日本語 IME と `herdr-core` PTY `IUTF8` 残存課題の調査

- **IME 品質維持**:
  - `TerminalViewport.tsx` の `ui.isComposing` 追跡とプライバシー不変条件（`e.data` 非記録）を維持。
  - `src/terminal/keyboard.ts` の `isImeComposingEvent()` が `storeIsComposing || event.isComposing || event.keyCode === 229 || event.key === "Process"` を検査し、変換中のアプリショートカットおよび macOS Option-as-Meta の誤作動を防ぐ。Option-as-Meta の実際のリスナーにもストアの変換状態を渡す。
- **`herdr-core` v0.1.22 PTY `IUTF8` 調査結果**:
  - `src-tauri/tests/herdr_integration.rs` の `pty_preserves_japanese_bytes_with_canonical_utf8_erase` で再確認した通り、固定版 `herdr-core` v0.1.22 は Unix `openpty` 呼び出し時に初期 `termios` を指定しないため、macOS の新規 PTY は `-iutf8` で起動する。
  - `herdr-core --help` および `herdr-core run --help` を確認し、v0.1.22 の CLI / RPC に `termios` や `IUTF8` を指定するオプションが存在しないことを確認した。
  - **影響範囲の切り分け**:
    - `fish`、`zsh`（zle）、`bash`（readline）、`nvim`、および TUI アプリは非カノニカル（raw）モードで動作し、UTF-8 文字単位の削除をユーザー空間で行うため、`-iutf8` の直接影響を受けない。
    - 影響を受けるのは、`cat`・`read`・readline 非使用の対話スクリプト等の**カノニカルモード（`icanon`）**で Backspace（`0x7f`）を押した場合であり、カーネル行規律が 1 文字（3 バイト）ではなく末尾 1 バイトだけを削除して UTF-8 が破損する。
  - **方針**: 制約に従い上流への無断 Issue/PR 投稿や独自フォークへの切り替えは行わず、シェル起動 `argv` をラッパーで書き換えるハックも避ける（ログインシェル解決やカスタム `argv` の契約を壊すため）。真の修正先である上流 `herdr-core` の PTY 初期化（`IUTF8` 付与）の課題として引き続き追跡し、ユーザー向け回避策（カノニカル入力が必要な場合の `stty iutf8`）を明記する。

### 1.4 キーボード操作と Action Registry 接続

- **実装**: `src/terminal/keyboard.ts`, `src/App.tsx`, `src/terminal/wtermAdapter.ts`
- **アプリ操作と端末入力の分離**:
  - macOS では `Cmd+C`（選択あり時のみ `terminal.copySelection`）、`Cmd+A`（`terminal.selectAll`）、`Cmd+T`（`terminal.create`）、`Cmd+Shift+W`（`terminal.close`）、`Cmd+=/- /0`（ズーム）、`Cmd+Shift+D`（開発用診断パネル切替）を `ActionRegistry`（`caller: "keybind"`）経由で実行する。
  - `Cmd+W` は Tauri の macOS 標準メニューでウィンドウを閉じる。タブを閉じる操作は `Cmd+Shift+W`。
  - `Cmd+V` は WKWebView のネイティブ `paste` イベントを `@wterm/dom` の `handlePaste` → `sendPaste`（ブラケットペースト対応）へそのまま通すことで、クリップボード権限ダイアログを回避しつつ安全に貼り付ける。
  - アプリのキーバインドは `Ctrl+C`、`Ctrl+D`、`Ctrl+[`、`Escape`、`Tab` などと競合しない。端末に選択があるときも、通常の `Ctrl+C` は選択を解除して wterm のキーエンコーダーへ渡す。選択中の `Escape` と `Escape` → `Tab` のフォーカス移動は wterm の動作に従う。
- **macOS Option-as-Meta 対応**:
  - macOS では `Option+b` / `Option+f` / `Option+d` / `Option+.` 押下時に `event.key` が合成文字（`∫`, `ƒ`, `∂`, `≥`）になるため、`translateMacOptionKey()` が物理 `event.code` から `ESC + <ascii>`（`\x1bb`, `\x1bf`, `\x1bd`, `\x1b.`, `\x1b\x7f` 等）へ変換して PTY へ送出する（`macOptionAsMeta: true` 既定・IME 変換中および Kitty keyboard protocol 有効時は無効）。

### 1.5 通常利用向け UI と PoC 診断 UI の分離

- **通常利用向け UI（`src/App.tsx`）**:
  - ターミナルを中心としたフルブリード画面（`.wterm.wterm-host` の角丸・ドロップシャドウを除去し最小パディングへ調整）。
  - 最小限の上部バー（タブ切替・閉じる・`+` 新規ターミナル・`JA/EN` 言語切替）のみを表示し、PoC 専用ボタンや右サイドバーは既定で非表示（`ui.diagnosticsPanelVisible: false`, `ui.sidebarVisible: false`）。
- **開発用 PoC 診断ドロワー（`src/poc/PocDiagnosticsDrawer.tsx`）**:
  - Phase 0 の描画エンジン切替、1/5/10 端末生成、日本語/VT/自動応答/大量出力プローブ、ベンチマーク実行、自動応答抑制・非表示描画停止トグル、診断ログを隔離して保持。
  - `Cmd+Shift+D`（非 macOS は `Ctrl+Shift+D`）、`app.toggleDiagnostics` アクション、または開発時クエリ `?poc=1` で表示できる。

---

## 2. 自動テスト・ビルド検証結果

対象 worktree: `/Users/k25012kk/ghq/github.com/suzuuuuu09/uiterm-phase2`（ブランチ `feat/phase2-terminal`）

| 検証項目 | コマンド | 結果 |
| --- | --- | --- |
| TypeScript 型検査 | `pnpm typecheck` | **合格**（0 errors） |
| Lint | `pnpm lint` | **合格** |
| Vitest（単体・統合・回帰） | `pnpm test -- --run` | **合格**（11 ファイル、51 件。Phase 0/1 の既存回帰を含む） |
| Rust テスト | `cargo test --locked --manifest-path src-tauri/Cargo.toml` | **合格**（unit 12 + `herdr_integration` 5 + `supervisor` 3 = 20 passed） |
| Clippy | `cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` | **合格**（0 warnings） |
| CLI 互換性（隔離 PTY） | `cargo test --locked --manifest-path src-tauri/Cargo.toml installed_cli_compatibility_in_isolated_pty -- --ignored` | **合格**（`fish` / `nvim` / `codex --version`） |
| リリースビルド | `nix develop --no-write-lock-file --command pnpm tauri build --bundles app` | **合格**（`uiterm.app` 3.66 MiB / Vite+ 警告 0 件） |
| Nix Flake 静的評価 | `nix flake check --no-build --all-systems --no-write-lock-file` | **合格**（`aarch64-darwin` / `aarch64-linux` / `x86_64-linux`） |

| macOS 実ウィンドウ | リリース .app を隔離 HOME / XDG で起動 | CoreGraphics でウィンドウ表示を検出。各試行で専用 core・holder・fish の起動と終了を観測。 |

### 独立レビューで修正した点

| 判定 | 問題と修正 | 回帰検証 |
| --- | --- | --- |
| 修正必須 | Option キーのキャプチャがストアの IME 状態を見ていなかった。変換を追跡するコールバックを実際のリスナーへ渡し、入力欄の blur で変換状態を解除する。 | アダプターのリスナーで IME 中の不送信、変換外の 1 回送信、解放を確認。既存の wterm 入力テストで IME 確定文字列の重複と Enter の誤送信を検証。 |
| 修正必須 | wterm の選択が残ると Ctrl+C が PTY へ届かない。キャプチャ時に端末の選択を解除し、wterm の通常エンコードに委ねる。コピー対象は端末内の選択に限定した。 | 選択解除とイベント通過をアダプターのリスナーで確認。 |
| 修正必須 | Tauri 標準メニューの `Cmd+W` はウィンドウを閉じる。タブを閉じる割り当てを `Cmd+Shift+W` へ変更した。 | 2 つの組み合わせの判定をテスト。実打鍵は手動検証。 |
| 修正必須 | DOM と herdr-core の両方がフォーカス通知を生成できた。FOCUS フレームに統一し、非アクティブ時のタブ切替や再アタッチも考慮する。 | フォーカス IPC の重複、非アクティブ時のタブ切替、復帰時の通知を検証。 |
| 修正必須 | herdr-core の終了シグナルが数値なのに Rust 側は文字列を要求し、終了済み端末を含む一覧が失敗した。Rust の変換境界と CLOSE デコーダーで正規化し、既存 IPC の文字列型を維持。UI はシグナルによる終了を表示する。 | 修正前の一覧取得失敗を再現。隔離 core に対する停止・一覧取得と、数値シグナルを含む CLOSE の回帰を追加。 |
| 修正必須 | 再起動の再入が進行中の起動を中断し、切断後にイベント購読が失われたままになる。再入を拒否し、失敗時は再起動対象を保持する。端末生成時に購読を確保し、切断・セッション消失を画面に表示する。 | モック IPC で切断後の再購読、再起動の連打、新セッションの状態反映を検証。 |
| 修正必須 | クリップボード読み込み中の再起動で、新しいシェルへ貼り付ける競合があった。読み込み前後で同じセッションか照合する。 | 読み込み中に再起動しても新セッションへ送信しないことを検証。 |
| 修正必須 | DOM のカーソル形状属性は常に存在するため、アプリの既定形状を適用する CSS が一致しなかった。既定は VT の `auto` とし、明示した形状を両描画経路のカーソル取得に反映する。 | 既定・強制・解除のカーソル形状をアダプターで検証。 |
| 修正推奨 | 自動応答の診断は種別・バイト数に限定し、未使用のセル計測 API と型を削除。 | TypeScript、Lint、既存の送信分離・入力プライバシーの回帰テストを実行。 |

Action Registry の引数検証を維持し、出力バイトは Zustand を経由しない。Phase 2 でネイティブの権限、CSP、ナビゲーション制限、生成 IPC DTO、依存ロックに差分はない。Rust の権限・入力検証・クレジット・古いアタッチの拒否・監視プロセスの回帰テストが通った。GUI 経由の再起動クリックと core 強制終了後の UI は手動検証に残る。

---

## 3. 性能測定結果（macOS arm64・リリースビルド）

### 3.1 配布容量・起動・アイドル

macOS arm64（Darwin 24.5.0）。Phase 0 `5ea6c3d`、Phase 1 `005f4b1`、最終修正後の Phase 2 を同じ Nix シェルでリリースビルドした。外部の herdr-core は固定版 `0.1.22`（6,603,232 B）。各版を隔離 HOME / XDG、既定 1000×700 ウィンドウ、wterm DOM と fish 1 端末で 3 回起動した。fish の起動引数は各版の既定に従った。起動時刻は CoreGraphics がウィンドウを検出した時点。足し合わせた RSS と `footprint` の Summary は別指標。4 秒以降で専用ソケットを検出したサンプルの試行内中央値を求め、さらに 3 試行の中央値を取った。各版の既定 UI が異なるため端末グリッドは揃えていない。

| 指標 | Phase 0 | Phase 1 | Phase 2 |
| --- | ---: | ---: | ---: |
| `.app` 論理サイズ | 4,422,468 B | 3,817,236 B | 3,833,748 B |
| `.app` + core | 10.51 MiB | 9.94 MiB | 9.95 MiB |
| ウィンドウ表示まで | 178.4 ms | 188.6 ms | 166.9 ms |
| Summary Physical Footprint | 159.42 MiB | 163.46 MiB | 110.37 MiB |
| 合計 RSS | 233.92 MiB | 233.06 MiB | 225.69 MiB |

Phase 2 の配布サイズは Phase 1 より 16,512 B 増。起動時間は Phase 0 が 167.9–427.7 ms、Phase 1 が 176.3–386.5 ms、Phase 2 が 159.4–443.0 ms と重なり、速度差を判定できない。3 版とも最初の試行が遅かった。メモリは既定 UI 全体の観測値で、グリッドや WebKit.GPU の影響を含む。プロンプト表示時刻とログイン直後のコールド起動は未計測。全 9 試行で SIGTERM 後に観測した関連 PID の残存なし。

### 3.2 大量出力と回収

macOS 実ウィンドウに接続した専用 PTY で `python3 scripts/bench-app.py workload --mib 5 --seconds 30 --rate-kib 256` を実行した。入力には同じ専用ソケットを指定した `herdr-core send` を使い、PTY の出力から終了マーカーを取得した。この経路は WKWebView での打鍵やクリップボードを通らない。Phase 2 の 30 秒測定はキーバインドと終了シグナルの最終修正前。

| 指標 | Phase 0（45×40） | Phase 1（45×40） | Phase 2（82×46、2 試行） |
| --- | ---: | ---: | ---: |
| アイドル時 CPU（関連 PID の CPU 時間差 ÷ 10 秒、コア 1 個を 100% とする） | 5.1% | 5.2% | 3.1–3.4% |
| 負荷中の最大 Summary Physical Footprint（サンプル） | 290.47 MiB | 351.42 MiB | 410.40–422.61 MiB |
| 負荷中の最大合計 RSS（サンプル） | 377.56 MiB | 361.33 MiB | 432.03–441.06 MiB |

各版で出力マーカーとシェルの終了コード 7 を確認した。Phase 0 は負荷後の SIGTERM 試行で core と holder が残ったため、その試行の PID と専用ソケットを照合して終了した。Phase 1 と Phase 2 は観測した関連 PID の残存なし。アイドルの `ps %CPU` は試行によって大きく振れたため、表には Footprint 採取を挟まない区間の CPU 時間差を載せた。負荷中のメモリ増加は Phase 2 の 2 試行で再現した。Phase 2 の列数・行数が他の版と異なり、同一グリッドでの Phase 固有の退行は判定できない。

最終 Phase 2 で同じレートを 120 秒続けた 1 試行では、Summary Physical Footprint は 10 秒 396.0 MiB、30 秒 416.5 MiB、60 秒 452.8 MiB、90 秒 366.7 MiB、119 秒 379.9 MiB。出力直後は 356.0 MiB。開始後に増加したが、サンプルは単調増加しなかった。終了マーカー、終了コード 7、SIGTERM 後の関連 PID 解放まで確認した。30 分の傾向と、ウィンドウを残したままタブを閉じた場合の回収は未計測。

アイドル CPU は最終版の 120 秒試行前の 10 秒間に 24% を記録した。別の起動でメインプロセスの `ps %CPU` が高くなる現象は Phase 1（20.5%）と最終 Phase 2（32.3%）の双方で再現し、両版に 0.3% の起動もあった。高値を検出した直後の 3 秒間の `sample` は両版とも待機スタックが中心で、継続的な CPU 使用箇所や Phase 2 固有の原因は特定できなかった。

固定サイズのヘッドレス測定は `node --expose-gc scripts/bench-terminals.mjs` で実行した。120×40、履歴上限 2,000 行、各端末へ日本語と ANSI を含む 10,000 行（996.1 KiB）を書き込んだ。

| 端末数 | 10,000 行解析 | 書き込み後 RSS 増分 | 全端末破棄・GC 後の RSS 増分 |
| ---: | ---: | ---: | ---: |
| 1 | 9.3 ms | 4.97 MiB | 5.08 MiB |
| 5 | 9.3 ms | 24.97 MiB | 0.09 MiB |
| 10 | 9.4 ms | 49.78 MiB | 0.09 MiB |

ヘッドレスの回収結果は Node.js プロセスでの観測。WKWebView でウィンドウを残したままタブを破棄した場合の回収は未測定。

---

## 4. ユーザー向け手動検証チェックリスト（macOS arm64 実機）

自動テストでは確認できない視覚・対話操作について、以下の手順で検証する。

### 検証用の起動手順

```sh
cd /Users/k25012kk/ghq/github.com/suzuuuuu09/uiterm-phase2
qa_home="$(mktemp -d /tmp/uiterm-manual.XXXXXX)"
nix develop --no-write-lock-file --command env \
  HOME="$qa_home" XDG_CONFIG_HOME="$qa_home/config" XDG_STATE_HOME="$qa_home/state" \
  XDG_DATA_HOME="$qa_home/data" XDG_CACHE_HOME="$qa_home/cache" ZDOTDIR="$qa_home" \
  ./src-tauri/target/release/bundle/macos/uiterm.app/Contents/MacOS/uiterm
```

Neovim で次の自動コマンドを登録し、別アプリへ切り替えて戻ってから `:messages` を開く。1 往復につき「離脱」「復帰」が 1 件ずつ出ることを確認する。

```vim
:autocmd FocusLost * echomsg "離脱"
:autocmd FocusGained * echomsg "復帰"
```

### チェックリスト

1. **通常利用向け UI と診断パネル分離**
   - [ ] 起動直後に PoC 専用ボタンや右サイドバーが表示されず、シンプルなタブバーとターミナル画面のみが表示されること。
   - [ ] `Cmd+Shift+D` で開発用診断ドロワーが開閉でき、通常利用を妨げないこと。
   - [ ] 上部バーの `EN` / `JA` ボタンで UI 文言が日本語・英語へ切り替わること。
2. **基本ターミナル操作・コピー＆ペースト・リサイズ**
   - [ ] fish で `exit 7` を実行すると終了コード 7 が出ること。「再起動」を素早く 2 回押しても新しいプロンプトが 1 つだけ出ること。検証用の fish で `kill -KILL $fish_pid` を実行し、シグナル終了の表示と「再起動」を確認する。
   - [ ] マウスドラッグ・ダブルクリック（単語）・トリプルクリック（行）で選択し、`Cmd+C` でコピー、`Cmd+V` でペーストできること。
   - [ ] 複数行テキストを fish や Neovim へ `Cmd+V` ペーストした際、ブラケットペーストとして意図せず即時実行されないこと。
   - [ ] ウィンドウをドラッグでリサイズした際、`stty size` の行列数が追従し、画面崩れがないこと。
   - [ ] 2 タブを作り、`Cmd+Shift+W` が現在のタブだけを閉じること。最後に `Cmd+W` でウィンドウを閉じる。
   - [ ] `Cmd+=` / `Cmd+-` / `Cmd+0` でフォントサイズが拡大・縮小・リセットされ、セル幅・行列数が再計算されること。
3. **フォント・Nerd Font・文字幅（3 パターン検証）**
   - 診断パネル（`Cmd+Shift+D`）内の「フォント検証（開発用）」セクション（`app.setFont` 接続・正式なユーザー向け設定 UI は Phase 4）を使い、以下の 3 パターンそれぞれで「日本語・文字幅」プローブ、fish プロンプト、および Neovim（ステータスライン/ツリー/日本語編集）の表示崩れを確認する。
     | パターン | 入力例 | 確認目的 |
     | --- | --- | --- |
     | デフォルト | 「デフォルト」ボタン（`ui-monospace, "SF Mono", "Menlo", "JetBrains Mono"`） | 初期状態での表示・PUA/日本語フォールバック |
     | 普段 Ghostty で使用しているフォント | 例: `"UDEV Gothic 35NF"` や `"UDEV Gothic NF"` 等を入力して「適用」（サイズ `12.5` 等も調整可） | Nerd Font・日常利用時の表示、セル寸法・行間再計算 |
     | Menlo | 「Menlo」ボタン（`Nerd Font フォールバック` ON / OFF の両方を切替） | Nerd Font 非対応プライマリフォント時の PUA フォールバック・日本語フォールバック |
   - [ ] 各パターンで ASCII 40 桁と全角 20 文字の右端が揃い、半角カナ・曖昧幅・絵文字/ZWJ・結合文字（NFD 濁点等）が崩れないこと。
   - [ ] 各パターンで Nerd Font アイコン（Powerline・Git・Devicons 等）の表示位置と後続セルの整列、および Neovim のレイアウトが崩れないこと。
   - [ ] フォント切替直後に「セル寸法（`charWidth x rowHeight px`）」と `stty size` が再計算され、描画ずれが起きないこと。
4. **日本語 IME（macOS 標準 IME）**
   - [ ] `stty -echo; cat; stty echo` 実行中に `にほんごにゅうりょく` を変換し、確定 Enter で行が送信されず、次の通常 Enter 1 回で 1 行だけ確定出力されること（Ctrl+D で終了）。
   - [ ] 画面中央・右端・最下行で未確定文字列と変換候補ウィンドウの位置がカーソルに追従すること。
   - [ ] 変換中に `Cmd+T` / `Cmd+Shift+W` / `Cmd+C` / `Option+b` / `Backspace` / `Esc` / 矢印を押してもアプリショートカット誤作動や PTY 誤送信が起きないこと。
   - [ ] 日本語文字列を選択して `Cmd+C` → `Cmd+V` で 1 回だけ貼り付けられること。端末内で選択を残したまま `Ctrl+C` を押しても前景コマンドが中断されること。
5. **キーボード操作と fish / Neovim / Codex 互換性**
   - [ ] `Ctrl+C`, `Ctrl+D`, `Ctrl+R`, `Ctrl+A`, `Ctrl+E`, `Tab`, `Shift+Tab`, `Esc` が fish / Neovim で正確に動作すること。
   - [ ] `Option+b` / `Option+f` / `Option+d` / `Option+Backspace` が fish で単語移動・削除として動作すること（`∫` や `ƒ` が入力されないこと）。
   - [ ] `nvim --clean` で挿入モード、日本語変換、`Esc`、リサイズ、`:q!` を試すこと。フォーカスは後述の自動コマンドで重複を確認する。
   - [ ] Codex CLI で日本語プロンプト入力・変換確定・取消・出力描画が正常動作すること。
6. **長時間利用・終了時クリーンアップ**
   - [ ] 大量出力（`python3 scripts/bench-app.py workload --mib 5` 等）後も描画・入力応答が維持され、スクロールバック上限（2,000 行）内で動作すること。
   - [ ] `Cmd+T` で負荷用タブを開き、大量出力後に `Cmd+Shift+W` で閉じる。元のタブを残した状態で、Activity Monitor の関連 WebContent のメモリを閉じる直前と 60 秒後で記録する。
   - [ ] アプリ終了後、今回起動した `uiterm`・監視プロセス・`herdr-core`・`holder`・シェルが残存せず、既存の Herdr セッションに影響がないこと。

---

## 5. 残存課題・後続 Phase への申し送り

| 優先度 | 項目 | 詳細 |
| --- | --- | --- |
| 高（手動検証） | Phase 2 UI での macOS 実ウィンドウ対話確認 | 上記チェックリスト（Nerd Font 目視、macOS IME 対話、fish/Neovim/Codex 対話、Option-as-Meta 実打鍵）は独立 QA では未実施。 |
| 中（上流依存） | `herdr-core` PTY 初期 `IUTF8` 未設定 | `herdr-core` v0.1.22 は macOS PTY を `-iutf8` で作成する。fish/nvim 等の raw モードには影響しないが、カノニカルモード（`cat` / `read` 等）のマルチバイト Backspace で末尾 1 バイトのみ消える。上流対応または回避方針を継続追跡する。 |
| 保留（Phase 4） | フォント・テーマ・キーバインドのユーザー設定 UI / TS 設定接続 | Phase 2 では内部 API（`app.setFont`, `app.setCursor`, `TerminalTheme`, `macOptionAsMeta`）までを整備した。永続化・TypeScript 設定ファイル・キーバインドカスタマイズは Phase 4 で接続する。 |
| 保留（Phase 6） | アイドル CPU の試行間変動と長時間（30 分 soak）WKWebView メモリ推移および Ghostty/Herdr 同一条件比較 | 30 分の連続出力・タブ破棄後の WKWebView 内メモリ回収・他端末との同一条件比較は Phase 6 で実施する。 |
| 保留（Phase 6） | NixOS / Hyprland（Wayland + WebKitGTK + Fcitx5）実機検証 | Nix Flake の Linux 静的評価のみ完了。実機ビルド・Wayland・Fcitx5 動作は Phase 6（MVP 完成条件）として保留。 |

ネイティブ ACL の拡張、追加のエミュレーターや依存の導入、PTY 起動ラッパーによる回避は対応不要と判断した。現行の IPC と描画経路で Phase 2 の確認を続ける。
