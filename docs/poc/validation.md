---
description: "uiterm Phase 0 の独立検証と移行判定"
---

# Phase 0 独立検証

## Phase 1 への移行判定

**現時点は保留。** Tauri 2・固定版 herdr-core・wterm DOM + libghostty を Phase 1 の候補として維持する。移行前に、QA と実装整理を統合したビルドで macOS の日本語 IME、WebView を含む端末操作・再同期・終了、および正式性能測定を確認する。初回入力や確定 Enter に回避できない問題があれば描画器の選定を見直す。NixOS / Hyprland の実機確認は今回の移行判定から外し、MVP 完成条件に残す。[ロードマップ](../plan/roadmap.md) [MVP](../plan/mvp.md)

2026-11-26 に Apple Silicon、macOS 15.5（24F74）で確認した。対象は `916815e` を基点とした `codex/phase0-qa` の差分。herdr-core は Flake に固定された `0.1.22`、Tauri は Cargo lock の `2.12.7`、wterm は pnpm lock の `0.6.0`。別作業の描画整理は含めていない。統合時は `App.tsx` の作成・破棄の競合対策、`sessionManager.ts` のアタッチ管理、`poc.test.ts` の回帰テストを整理後の描画器・タブ操作と照合する。

| 確認対象 | 実行結果 |
| --- | --- |
| Rust | 単体6件・統合6件が通過。Clippy は `-D warnings` で通過。 |
| フロントエンド | 型検査・Lint・Vitest 10件・ビルドが通過。WASM と合成 IME イベントは Node 上で検証。 |
| CLI / TUI | 隔離 PTY で fish 4.9.3 の日本語入出力、Neovim 0.12.6 の日本語挿入・終了、Codex 0.170.0 の `--version` を確認。Codex の対話と認証は手動確認に残る。 |
| macOS 配布 | `pnpm tauri build --bundles app --ci` が成功。CLI の暫定表示は 13.76 MiB（外部 core を含まない）。macOS 14.0 向けの Rust ライブラリを 11.0 ターゲットにリンクする警告が残る。配布対象の最小 OS とツールチェーンを揃える案をビルド担当に申し送る。 |
| NixOS | 2アーキテクチャの Flake 評価と配布 core のハッシュ・ELF を確認。実機実行は保留。 |


## IPC・ライフサイクル

| 項目 | 再現・確認結果 | 対応 |
| --- | --- | --- |
| 初回アタッチと ID | 応答直後に DATA、RESIZE、CLOSE が届き、コマンド呼び出し側がまだ応答を取得していない順序を単体テストで再現。旧方式では、一時キューの再配信中に後続フレームが先に届く余地があった。 | ソケット読み取り側で RPC 応答とストリーム登録を一緒に処理する。0、u32 範囲外、既存のワイヤーチャンネルを拒否し、接続をまたいでアプリ内の ID を再利用しない。 |
| CREDIT と ACK | 隔離コアの端末で ACK を止めると、256 KiB でデータが止まった。別端末の応答と制御 RPC は継続し、正しい CREDIT で出力が再開した。過剰な ACK は拒否した。 | Rust は未 ACK バイト数を検証する。wterm の同期 VT 処理の後に合計 16 KiB 以上を ACK。イベント購読にも WebView 受信後の ACK を追加した。 |
| 自動応答とユーザー入力 | 固定版コアで DATA、AUTOMATIC_INPUT と OSC 7501 の状態遷移を確認。単体テストで未交渉の自動応答が DATA として送信されないことを確認した。 | 交渉済みの経路でのみ kind 6 を使う。従来の上流互換フォールバックは人間の入力として扱われるため、未交渉時は構造化エラーを返す。 |
| リサイズと再同期 | バックエンド起点のリサイズと同一サイズの再通知をモックで再現。別アタッチに切り替えた後に古いストリームから届いた出力が破棄されることを確認した。 | コールバックをアタッチごとに無効化できるようにし、同一タブの再同期を直列化する。生きたコアへの再接続では、実ソケット上で旧 ID の拒否と既存画面のリペイントを確認した。閉じたアタッチの入力と ACK は廃棄する。生きているコアへの再接続と、コア自体の異常終了は扱いが異なる。 |
| バッファ・競合 | イベントは RPC 応答前に到着でき、JSON 通知はフレームをまたいで分割される。単体テストで前後のフレームを再構成し、CREDIT 対象のバイト数を検証した。 | 接続内でキュー待ちを行わず、端末・イベントのキューと受信バイト数に上限を置く。キュー超過や読み書き障害で接続を閉じ、保留 RPC と全ストリームへ通知する。 |
| 作成失敗と終了 | アタッチ失敗、作成進行中の全タブ破棄、古いアタッチの使用をテストした。隔離コアの通常停止でソケットを含む作業ディレクトリの解放を確認した。 | マウント済みアダプターと作成済み端末をロールバックする。コアはアプリ管理下の子プロセスとして立ち上げ、通常終了時は公開 `server stop` で停止し、終了を待つ。 |
| エラーと上限 | コード上でキュー確保待ち・RPC・hello を確認した。クレジット上限だけでは RESIZE、イベント、送信側の保留 RPC は制約されない。 | 入力ヘッダー、グリッド寸法、送信フレーム長を検証する。アタッチ前の ACK は合算する。入力はアタッチ後も順番に IPC へ送り、待機分を256件、送信中も含めて1 MiBを上限とする。切断した接続へ自動再送しない。 |

[IPC 仕様](./interface.md) [Rust 統合テスト](../../src-tauri/tests/herdr_integration.rs) [herdr-core v0.1.22 wire 仕様](https://github.com/herdrdev/herdr-core/blob/v0.1.22/schema/README.md) [固定版コアの受信・送信処理](https://github.com/herdrdev/herdr-core/blob/v0.1.22/crates/herdr-core/src/server.rs)

Tauri の固定版 Channel は 1,024 バイト未満の Raw を JS 評価用の数値配列に変換し、より大きいフレームには fetch 経路を使う。フレームサイズによって転送コストが異なる。[Tauri 2.12.7 Channel 実装](https://docs.rs/tauri/2.12.7/src/tauri/ipc/channel.rs.html)

多重化の検証範囲は受信側で CREDIT が枯渇する場合。単一ソケット上での大きな入力フレームの直列送信、Tauri Channel の配信が滞った場合のメモリ、WebView が消えた場合の切断時刻は統合後の計測対象とする。アプリ側のタイムアウトで `terminal.run` の成否が不明になる場合は、安全に確認できる端末アドレスだけを後始末の対象とする。

## 日本語・PTY

### WKWebView と入力イベント

UI Events の仕様では、変換セッション内のキーイベントに `isComposing` が付く。WebKit には macOS で `compositionend` が確定 Enter の `keydown` に先行する履歴があり、`isComposing` だけでは判別できない既存 OS が対象に残る。修正は WebKit の上流に入った。今回の macOS 15.5 に含まれる WKWebView でのイベント順は手動試験で確認する。フォーカスを失ったときの `compositionend` に関する報告もある。[UI Events](https://www.w3.org/TR/uievents/#events-composition-key-events) [WebKit #165004](https://bugs.webkit.org/show_bug.cgi?id=165004) [WebKit #311717](https://bugs.webkit.org/show_bug.cgi?id=311717) [WebKit #164369](https://bugs.webkit.org/show_bug.cgi?id=164369)

固定版 `@wterm/dom` の入力実装は変換中フラグ、`isComposing`、`keyCode === 229` のいずれかで端末キーへの変換を抑える。合成イベントのテストでは候補操作中の Backspace、矢印、Ctrl+C と、`compositionend` 後の 229 の Enter が PTY 入力にならず、重複する `input` イベントも抑制された。次の通常 Enter は CR になった。将来のアプリ共通キーバインドも同じ保護が必要になる。元の PoC のキャプチャリスナーはイベントの観測だけを行う。[wterm](https://github.com/vercel-labs/wterm) [日本語の回帰テスト](../../src/terminal/__tests__/unicode.test.mjs)

固定版 libghostty WASM に UTF-8 を1バイトずつ渡し、ASCII、半角カナ、全角英字・漢字、曖昧幅の `α`、結合文字 `é`、ZWJ 絵文字 `👩‍💻` のセル境界を確認した。この設定では `α` は1セル。East Asian Width の曖昧幅は文脈に依存する。結合列の区切り、セル幅、フォールバックで選ばれる書体はそれぞれ別の性質なので、字形と描画位置は実ウィンドウで確認する。[Unicode UAX #11](https://www.unicode.org/reports/tr11/) [Unicode UAX #29](https://www.unicode.org/reports/tr29/)

### カノニカルモードの Backspace

隔離した macOS PTY は起動時に `-iutf8` だった。`stty icanon -echo erase '^?' -iutf8` で `A日本<DEL>B<LF>` を送ると、読み取ったバイト列は `41 e6 97 a5 e6 9c 42` になり、`本` の末尾1バイトだけが消えた。`iutf8` を指定した場合は `41 e6 97 a5 42`（`A日B`）だった。このテストはカーネルの行編集を対象とする。画面上の全角エコー、結合文字単位の削除、raw モードのアプリ側行編集は個別に確認する。

修正先は **herdr-core**。固定版 `6e29846` と調査時点の上流 HEAD `329cb66` の Unix `spawn_pty` は初期 termios を指定せずに `openpty` を呼ぶ。上流で slave 側の既存 termios を取得し、対応 OS の UTF-8 PTY で `IUTF8` を追加して子プロセス起動前に設定する。固定値で他のフラグまで上書きする必要はない。macOS と Linux で既存設定の保持と上記バイト列を回帰テストする。追跡用の上流 Issue は調査時点で特定できなかった。[固定版の PTY 初期化](https://github.com/herdrdev/herdr-core/blob/v0.1.22/crates/herdr-core/src/platform/unix.rs) [上流 HEAD の同箇所](https://github.com/herdrdev/herdr-core/blob/329cb66ebce5eca3d917fcf7364a099046979b49/crates/herdr-core/src/platform/unix.rs) [Apple XNU tty.c](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/tty.c) [Linux n_tty.c](https://github.com/torvalds/linux/blob/master/drivers/tty/n_tty.c)

uiterm が管理するのはシェルとロケールの初期値。指定がない場合は `getpwuid_r(geteuid())` の `pw_shell` を優先して、取得不可時は `SHELL` を参照する。ログインシェルとして登録された実行ファイルを選び、`-l` は付けない。空または欠落した `LANG` に対して UTF-8 の `LC_CTYPE`、macOS の `en_US.UTF-8`、Linux の `C.UTF-8` を候補とする。呼び出し元が指定した `LC_ALL`、`LC_CTYPE`、`LANG` は保持する。対話シェルの行編集やロケール補完を行っても、カーネルの `IUTF8` フラグは独立している。[GNU libc ユーザー検索](https://www.gnu.org/software/libc/manual/html_node/Lookup-User.html) [uiterm の端末作成コマンド](../../src-tauri/src/commands.rs)

### macOS 日本語 IME の手動確認

1. 統合後のコミットを記録して `.app` を作成する。テスト用の macOS ログインセッションで、日本語入力ソースとフォントを確認する。ビルドに使った `nix develop` 内で `./src-tauri/target/release/bundle/macos/uiterm.app/Contents/MacOS/uiterm` を起動する。ライブ端末と wterm DOM を選び、OS ビルド、WebKit バージョン、入力ソース、文字サイズ、端末の `locale charmap` と `stty -a` を記録する。
2. 確定 Enter を検証するため、端末で `stty -echo; cat; stty echo` を実行してから `にほんごにゅうりょく` を変換する。変換確定 Enter で行がまだ表示されず、次の通常 Enter 1回で確定文字列が1行だけ表示されることを確認する。Ctrl+D で戻す（行が残っていれば空行で再度押す）。
3. 端末の中央、右端、最下行で未確定文字列と候補ウィンドウの位置を確認する。候補選択中の矢印、Space、Backspace、Ctrl+C、Esc が誤って PTY 入力やタブ操作にならないことを確かめる。変換取消、別タブへの切替、フォーカス復帰、再変換を試し、確定文字の消失・二重入力を記録する。診断ログの 229 はイベント順の手掛かりとして使う。
4. 日本語プローブに加え、`printf '12345678|\nあいうえ|\nAＡｶ日αé👩‍💻!|\n'` を表示し、ASCII 8桁と全角4文字の右端を比較する。半角カナと曖昧幅文字のセル位置、絵文字・結合文字の重なり、豆腐字、改行境界、選択・コピーの文字列を確認する。端末の基本フォントに日本語グリフがない場合も同じ試験を行い、実際に使われたフォールバックを記録する。
5. テスト端末だけで `stty iutf8` の前後を比較し、シェルのプロンプトと `cat` のカノニカル入力で `日本語` を1文字ずつ削除する。続けて `fish --no-config --private -i`、`nvim --clean -n -i NONE` で文字入力・削除・リサイズと終了を確認する。Codex はまず `codex --version` を確認し、対話はテスト専用設定・認証がある場合に限って日本語プロンプトの確定・取消・描画を試す。
6. 一度タブを再同期し、出力とカーソルが戻ることを確認する。端末をすべて閉じてからアプリを終了し、その実行に属する core・holder・シェルの PID が消えることを確認する。確定操作のログや画面にはテスト用の文字列だけを使う。

## NixOS / Hyprland

### 静的検証結果

- 固定された nixpkgs `2786a2a7` に対して `x86_64-linux` と `aarch64-linux` の `devShells.<system>.default.drvPath` と `packages.<system>.herdr-core.drvPath` を macOS で評価した。両方の配布 tarball の SHA-256 が Flake と一致した。ファイル形式は x86-64 が static PIE の ELF、aarch64 が静的リンク ELF。ビルド・実行は実機で確認する。
- `nix flake check --no-build --all-systems` は `x86_64-darwin` で停止する。固定先の nixpkgs 26.11 が Intel Mac を対象外にしている。MVP の Apple Silicon 範囲に合わせて列挙から外す案をビルド担当に申し送る。[nixpkgs 26.11 リリースノート](https://nixos.org/manual/nixpkgs/unstable/release-notes#x86_64-darwin-26.11)
- Cargo lock の Wry 0.57.4 は GTK3 系と WebKitGTK 4.1 系の Linux 依存を持つ。Flake には `webkitgtk_4_1`、GTK3、libsoup 3、D-Bus、Wayland、xkbcommon と pkg-config がある。`bundle.targets` の `app` は macOS 用で、Linux 向けパッケージやラッパーは別途必要になる。ビルド担当には実機の pkg-config、GSettings スキーマ、動的リンク、NixOS 用ラッパーを確認するよう申し送る。[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) [Wry v0.57.4](https://github.com/tauri-apps/wry/blob/wry-v0.57.4/Cargo.toml)
- Flake の `GDK_BACKEND=wayland,x11` は X11 にフォールバックできる設定。GTK3 の Wayland で使う text-input-v3 と Fcitx5 の input-method-v2 の組み合わせを実機で確認する。Mozc は入力メソッド側の追加コンポーネント。標準の Wayland 経路で動作しない場合だけ、当該プロセスに `GTK_IM_MODULE=fcitx` を設定した再試験を行う。[Fcitx 5 on Wayland](https://fcitx-im.org/wiki/Using_Fcitx_5_on_Wayland) [Fcitx5 Mozc](https://github.com/fcitx/mozc) [Hyprland #5815](https://github.com/hyprwm/Hyprland/issues/5815)
- Unix Socket、`mkdtemp`、`getpwuid_r` は両 OS のコード経路。core の `HC_SOCKET` と XDG ディレクトリを隔離し、固定版の Linux 実装が私有 `XDG_RUNTIME_DIR` のとき systemd ユーザーサービスを起動しない経路を使う。UI から作る端末に、呼び出し元の Wayland ソケットとランタイムパスを明示して戻す。glibc の `C.UTF-8` 可用性、NSS のシェル解決、グループ終了時の holder・シェルの実挙動は NixOS で確認する。[固定版の systemd 起動条件](https://github.com/herdrdev/herdr-core/blob/v0.1.22/crates/herdr-core/src/platform/systemd.rs) [固定版のソケット規約](https://github.com/herdrdev/herdr-core/blob/v0.1.22/crates/herdr-core/src/paths.rs)

### 実機への申し送り

```sh
nix develop --no-write-lock-file
pkg-config --modversion webkit2gtk-4.1 gtk+-3.0 libsoup-3.0
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test -- --run
cargo test --locked --manifest-path src-tauri/Cargo.toml
env -u GTK_IM_MODULE GDK_BACKEND=wayland pnpm tauri dev
```

NixOS と Hyprland のバージョン、GPU、WebKitGTK、Fcitx5、Mozc、`locale charmap` を控える。`hyprctl clients` の uiterm ウィンドウで `xwayland: 0` を確認し、同じウィンドウで日本語の未確定表示、候補、Enter、Backspace、フォントを試す。必要時に `WAYLAND_DEBUG=1` のログから text-input の接続を確認する。候補が出ない場合は Fcitx5 デーモンと Mozc の選択、フロントエンドの接続、GTK IM モジュール経路を順に確認する。10端末の終了後に対象 core・holder とシェルが終了し、既存の Herdr セッションが維持されることを記録する。

## 性能測定

[bench-app.py](../../scripts/bench-app.py) は Python 標準ライブラリと macOS の `ps`、`lsof`、`footprint` を使う。アプリの主 PID とその子孫、対象の Unix Socket を保持する core・holder とその子孫を採取する。PID、開始時刻、実行ファイル名を残し、個別に割り当てた WebKit ヘルパーを含められる。起動時に増えた WebKit ヘルパーを採取する `--webkit-delta` は他の WebKit アプリの起動を止めたテストセッションで使用する。既存の PID をサンプルに含める場合は `--include` を繰り返し指定する。

RSS はカーネルの各 PID の値を合計した参考値で、共有ページが重複する。各 PID の `phys_footprint` と `footprint` の複数 PID 計測の Summary Footprint は別々に保存する。Summary はツールが定義する集合の値であり、`phys_footprint` の和との一致を前提にしない。PID の退出やアクセス制限で欠測したサンプルは `footprint_status` を確認する。CPU は `ps` の平滑化された %CPU の合計。短いピークには複数回のバーストを使う。[Apple のメモリ計測解説](https://developer.apple.com/videos/play/wwdc2021/10180/)

### 再現手順

1. 統合後の release `.app`、同梱外の core バイナリと、比較用の Ghostty・Herdr を用意する。既存ユーザーのプロファイルに触れないテスト用ログインセッションで実施し、アプリの PID、バージョン、コミット、シェル、フォント、グリッドサイズ、スクロールバック、表示端末数、OS と電源状態を記録する。配布容量は `.app` の論理ファイルサイズの合計と、同梱外で必須の CLI / core を別々に取る。同じ形式の配布アーカイブがある場合はそのファイルサイズを `--extra` で記録する。
2. 初回はログイン後に比較対象を起動していない状態で `--label first-after-login`、続く試行は GUI を正常終了して PID が消えてから `--label warm` を指定する。初回をアプリごとに3回、ウォームを5回以上繰り返し、中央値と範囲を残す。起動時間の自動計測点はプロセス起動から可視の初回ウィンドウまで。オプション `--wait-ready` はプロンプトを人間が確認した時刻の上限で、反応時間を含む。既存レポートのプロンプト到達値との比較では計測点を明記する。
3. 各アプリで同じサイズの端末と同じシェルを用意し、表示1端末、残りを非表示タブに揃える。端末1・5・10個で安定後30秒を `idle-1`、`idle-5`、`idle-10` として記録する。uiterm で起動時に取得した `core_socket` と `webkit_delta_pids` を後続の PID 計測に引き継ぐ。候補 PID の帰属を確認できなかった試行は全体比較から外す。
4. 表示中の端末から同じ `workload` を実行する。無制限の5 MiB出力で終了マーカーまでの所要時間を取り、負荷サンプリングは低頻度で見逃さないよう、同じ間隔でバーストを10回繰り返す。`soak` は30分、一定レートで実行する。負荷中は開始時・安定後・終盤の Footprint、RSS、CPU と端末ごとの表示状態を記録する。
5. `post-close` を60秒採取し始めてから端末をすべて閉じる。アプリが最後のタブとともに終了した場合は、`app_alive` と関連 PID の変化を確認する。各試行の PID と時刻を照合し、shell と holder の残存および Footprint の戻り方を比較する。

```sh
python3 scripts/bench-app.py self-test
nix develop --no-write-lock-file --command sh -c '
  python3 scripts/bench-app.py measure \
    --app src-tauri/target/release/bundle/macos/uiterm.app \
    --extra "$HERDR_CORE_BIN" --label warm --stage idle-1 \
    --wait-ready --core-socket auto --webkit-delta \
    --condition "release、wterm DOM、1端末、表示1、120x40"
' > uiterm-idle-1.jsonl

# 同じアプリ内で端末を5個に増やしてから計測端末で実行する。
# APP_PID、WEBKIT_PID、PRIVATE_SOCKET は起動時ログで確認した値に置き換える。
python3 scripts/bench-app.py measure \
  --app src-tauri/target/release/bundle/macos/uiterm.app \
  --pid "$APP_PID" --include "$WEBKIT_PID" --core-socket "$PRIVATE_SOCKET" \
  --stage idle-5 --condition "release、wterm DOM、5端末、表示1、120x40" \
  > uiterm-idle-5.jsonl

# 各アプリで同じ作業ディレクトリを選び、端末から実行する。
python3 scripts/bench-app.py workload --mib 5
python3 scripts/bench-app.py workload --mib 0 --seconds 1800 --rate-kib 256
```

このブランチでの実行はスクリプトの自己テストと収集経路のスモークまで。正式な負荷・長時間・アプリ間比較は統合後に実施する。既存 `poc_bench` は Framed Mode のバックエンド測定、`bench-terminals.mjs` は Node 上の VT/WASM 測定、UI 内蔵のバーストは描画器への直接入力。いずれも同じアプリ全体測定としては扱えない。

## セキュリティ・設計の残課題

| 優先度 | 課題と根拠 |
| --- | --- |
| 高（配布前） | `core:default` は独自コマンドの許可範囲を絞らない。現状 CSP が `null` で、端末起動 IPC に `argv` / `line` がある。ビルド担当への変更案は `AppManifest::commands` とウィンドウ `main` 専用の許可リスト、外部ページの別 WebView、WASM と IPC に必要な接続先だけを許容する CSP。開発サーバー用と配布用を分け、WASM・スタイル・Tauri IPC の結合テストで確認する。 |
| 高（配布前） | `HERDR_CORE_BIN` と絶対パスの PATH エントリーを起動元の信頼境界とした。固定版を sidecar として同梱するか、外部依存としてバージョン・由来を検証する方針を決める。foreground `serve` と macOS の直接起動フラグは固定版の内部インターフェースであり、更新時には両 OS で再検証する。 |
| 高（Phase 1） | TypeScript は現在のユーザー権限で動く実行可能コード。PoC の WebView からはサンプルだけを許可し、Node の stderr を放置したパイプによる停止と無応答を防いだ。ワークスペース自動検出からの実行は本人が明示的に信頼したパスに限定する。プロジェクトに入っただけで読み込む設計を避け、Node と子孫プロセスの取り扱いも決める。 |
| 高（Phase 1） | 0700 の一時ディレクトリは別ユーザーからのソケット到達を抑える。同一 UID のプロセスとは権限を共有する。通常終了をテスト済み。GUI の強制終了、コアのクラッシュ、起動とアプリ終了が重なった場合には holder・シェルが残り得る。起動インスタンスにひも付けた復旧と、停止が確認できないときの所有情報・状態の保持を定義する。 |
| 中（Phase 1） | コア切断中のリクエスト成否や再アタッチ対象の消失を UI で明示する。イベント購読の snapshot / missed / seq によるモデル更新、書き込み順と送信キューの遅延、複数タブの復旧は統合後に継続検証する。 |
| 中（Phase 1） | PoC の IME 診断ログには未確定・確定文字列が入り得る。配布向けは文字列を保存せず、診断を明示的な操作に限定する。代替レンダラーは非同期 `write()` の場合に ACK と自動応答を分類する契約が必要。固定版 wterm DOM は候補位置とフォント描画を手動確認する。 |
| 中（ビルド担当） | 既存依存でブラウザー向けに `node:url` と `node:fs/promises` の externalization、500 kB 超チャンク警告を再現した。候補描画器を含めたビルドとの差をリリースファイルと起動後の Footprint で測る。バンドル縮小からメモリ削減量を推定しない。 |

[Tauri Capabilities](https://v2.tauri.app/security/capabilities/) [Tauri CSP](https://v2.tauri.app/security/csp/) [herdr-core のセキュリティ前提](https://github.com/herdrdev/herdr-core/blob/v0.1.22/docs/security.md)

## 既存レポートとの照合

| 既存記録 | 判定 |
| --- | --- |
| 統合 §1.5 は macOS IME 確定を合格とし、§5・フロント §1.2・旧ロードマップは手動未完了とする。 | 過去の単一入力事例を参考記録として扱う。候補追従、取消、アプリ全体のキーバインド、統合後の回帰は未完了。 |
| 統合のアプリ全体 237.75 MiB と、Ghostty 48.47 MiB / Herdr 93.97 MiB。フロント報告の比較は Ghostty 約56.4 MB / Herdr 約63.1 MB + core 約12.6 MB。 | 集計プロセス・負荷・単位が揃っていない。共有メモリの重複も含み得る。現環境のバンドル情報は Ghostty 1.4.0、Herdr 0.1.16 だった。報告にある Herdr 0.9.5 は当時の実体・アプリ系統の再確認が必要。 |
| バックエンドの10端末合計 21.69 MiB と統合の 17.31 MiB。 | 観測したプロセス、待機時間、出力負荷が揃っていない。全端末のシェルを含む同一条件のスケーリングは再測定する。 |
| 統合の WASM 増分 5.2 / 23.4 / 46.1 MiB と解放率 99.5%。フロント表はアイドル増分が 7.95 / 4.97 / 9.77 MB。 | 元のデータと測定点が一致しない。後者の10端末負荷後は +89.50 MB、強制 GC 後は +0.45 MB だが、プロセス全体の一試行。WKWebView での解放率の根拠には使わない。 |
| 統合 §2.4 の20秒・約20 MiB の結果から「完全に有界」「リークなし」。 | 観測区間の core と holder の RSS 記録として扱う。スクロールバック上限から、WebKit、キュー、PTY、WASM の長時間メモリは推定できない。 |
| 配布 `.app` 13.73 MiB と必須 core 6.3 MiB、初回 1,274.2 ms とウォーム 534.1 ms。 | 過去の PoC 値。同梱区分、計測点、試行数を揃えて統合後に再計測する。 |

[バックエンド報告](./backend.md) [フロントエンド報告](./frontend.md) [統合報告](./integration.md)

## 再実行コマンド

```sh
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test -- --run
pnpm build
nix develop --no-write-lock-file --command cargo test --locked --manifest-path src-tauri/Cargo.toml
nix develop --no-write-lock-file --command cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
nix develop --no-write-lock-file --command cargo test --locked --manifest-path src-tauri/Cargo.toml installed_cli_compatibility_in_isolated_pty -- --ignored
nix eval --raw '.#devShells.x86_64-linux.default.drvPath'
nix eval --raw '.#packages.x86_64-linux.herdr-core.drvPath'
nix eval --raw '.#devShells.aarch64-linux.default.drvPath'
nix eval --raw '.#packages.aarch64-linux.herdr-core.drvPath'
```
