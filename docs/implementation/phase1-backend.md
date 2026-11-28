# Phase 1のRust・ネイティブ基盤

RustはTauri IPC、Unixソケット、端末プロセスの監視、OSのアカウント情報取得を担当する。ワークスペース、タブ、アクションの状態と操作はTypeScriptが管理する。Phase 0の端末通信とリペイントの契約は [IPC仕様](../poc/interface.md) を参照。

| 実装                                                              | 責務                                                              |
| ----------------------------------------------------------------- | ----------------------------------------------------------------- |
| `src-tauri/src/commands.rs`                                       | Tauri Commands、入力検証、出力・イベントのChannelsへの転送        |
| `src-tauri/src/app_state.rs`                                      | GUIインスタンスごとの監視プロセスと接続の遅延起動、再接続の直列化 |
| `src-tauri/src/herdr/{client,protocol,types}.rs`                  | herdr-core Framed Mode、RPC、ストリーム、DTO、構造化エラー        |
| `src-tauri/src/herdr/process.rs` と `herdr/process/supervisor.rs` | 固定版の起動、専用ソケット、終了監視、coreの復旧                  |
| `src-tauri/src/os.rs`                                             | Unixアカウントのデフォルトシェル取得                              |
| `src-tauri/src/config_runtime.rs`                                 | 開発時に明示的に有効化する、外部Node.jsとのPoC接続                |

## IPCとTypeScriptの統合

Commandsは端末の作成・操作、出力の消費通知、接続状態と購読開始時のスナップショットを扱う。端末出力は `Channel<Response>` のRawボディ、端末入力は `Request` のRawボディを使う。旧クライアント向けのJSON文字列入力も受理する。フレーム種別、ヘッダー、バイト順、イベント購読のJSONはPhase 0仕様を引き継ぐ。

アダプターはユーザー入力とVT自動応答を別のFramed Modeフレームで転送する。自動応答を分離できるかは `terminal_attach` の応答で判断する。クライアントは制御RPCを最長10秒待つ。入出力上限は受信フレーム16 MiB、送信フレーム1 MiB、送信キューと未応答RPC各64件、ストリーム最大128本、各出力ストリームの初期クレジット256 KiB。ACKは実際に消費したバイト数を返す。入力の構文エラーは `invalid_args`、旧ストリームに対する操作は `invalid_attach` を返す。端末作成では `argv` と `line` の同時指定、不正な環境変数、NUL、寸法の片側指定を拒否する。

切断後は `core_reconnect` の後に購読を作成し直し、新しい `seq` とスナップショットで状態を作り直す。`missed` を受けた場合もイベントに含まれるスナップショットを使う。端末の生存を確認し、新しいattach IDでアタッチしてリペイントを受け取る。旧attach IDや購読IDを再利用しない。タイムアウトした作成・終了の成否は確定しないため、端末一覧で照合してから次の操作を決める。

Rustが所有するJSON DTOの生成先は [src-tauri/bindings/ipc.ts](../../src-tauri/bindings/ipc.ts)。ts-rsはテスト依存で、通常の `cargo test` が再生成結果とファイルの一致を検査する。DTO変更と生成物を同じ変更に含める。

```sh
nix develop --no-write-lock-file --command env UITERM_UPDATE_BINDINGS=1 \
  cargo test --locked --manifest-path src-tauri/Cargo.toml --lib ipc_bindings_are_current
nix develop --no-write-lock-file --command \
  cargo test --locked --manifest-path src-tauri/Cargo.toml --lib ipc_bindings_are_current
```

TypeScript側の統合では、`src/ipc/generated/index.ts` の再エクスポート先を `../../../src-tauri/bindings/ipc.ts` に変更する。次に `src/ipc/types.ts` からその型を再エクスポートし、同名の手書き宣言を外す。これにより既存のimportパスを維持できる。Rawフレームのデコーダー、コマンド呼び出しのラッパー、UI向け型はTypeScriptに置く。herdr-coreが返す一部のイベントの `data` はRustでも任意のJSONなので、生成型では `unknown` になる。既存の詳細型を使う箇所では、イベント種別ごとにパースしてからUIのモデルへ渡す。`u64` は既存JSONと同じ `number` で表す。optionalキーとnull値を区別する。業務エラーは `{ code, message, hint? }`。Tauriでの権限拒否と引数のデシリアライズ失敗は別途文字列で返り得るため、フロントエンドのエラー境界で正規化する。 [ts-rsの型生成仕様](https://docs.rs/ts-rs/latest/ts_rs/trait.TS.html)

## プロセスの所有と終了

GUIは最初のネイティブ要求で、同じ実行ファイルを監視モードで起動する。監視プロセスが `mkdtemp` で作ったモード0700のディレクトリに、coreとholderのソケットおよびXDG設定・状態を置く。セッション名とディレクトリは起動ごとに割り当てる。既定のHerdrソケットには接続せず、実行ファイル名や古いPIDによる一括終了も行わない。0700は別UIDからのアクセスを制限する。同じUIDのプロセスおよび端末から起動したプログラムは、そのユーザーの権限を持つ。 [固定版のソケット・状態パス](https://github.com/herdrdev/herdr-core/blob/v0.1.22/crates/herdr-core/src/paths.rs)

| 状況                                      | 処理                                                                                                                                          |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| 正常終了、SIGTERM、SIGINT                 | Tauriの終了処理が監視パイプを閉じる。監視プロセスが専用ソケットへ `server stop` を送り、成功応答とcoreの終了を待つ。                          |
| GUIのクラッシュ、SIGKILL、起動中のGUI終了 | OSがパイプを閉じる。別プロセスグループの監視プロセスがEOFを受け、同じ停止処理を行う。                                                         |
| coreのクラッシュ                          | 専用状態を使ってcoreを起動し直す。holderが先に復旧させた場合は専用ソケットが再び開くのを待つ。GUIは再接続と再アタッチで生存端末を復旧できる。 |
| coreの初回起動失敗                        | coreをGUIへ公開する前に失敗を返し、起動した子を終了させて専用ディレクトリを解放する。                                                         |
| 停止を確認できない場合                    | 専用ディレクトリを保持する。監視プロセス自体も失われた場合や、停止処理がタイムアウトした場合、次回起動は新規インスタンスを作成する。          |

停止未確認のインスタンスを調査する際は、監視プロセスのPIDを含む `st/herdr-core/ut-app-…` の状態と、同じ専用ディレクトリのソケットを照合する。帰属を確認できた場合だけ、そのインスタンスの固定版・ソケット・セッション・XDG設定を明示してcoreを復旧し、`server stop` で応答と終了を確認する。古いディレクトリを起動時に走査して自動回収する処理は設けていない。端末から独立したセッションに離脱し、coreの追跡用環境も除去したプロセスには、固定版の追跡にも限界がある。 [固定版のholder監視](https://github.com/herdrdev/herdr-core/blob/v0.1.22/crates/herdr-core/src/holder.rs) [固定版の停止処理](https://github.com/herdrdev/herdr-core/blob/v0.1.22/crates/herdr-core/src/runtime.rs)

## WebViewと設定コードの信頼境界

Capabilitiesはローカルの `main` WebViewに `terminal-ipc` のCommandsだけを許可する。メイン画面の遷移先は組み込みアプリと開発時のViteオリジンに限定し、ポップアップを閉じる。Tauriの組み込みChannel転送を使い、シェル・ファイル操作プラグインの権限は付与しない。配布用CSPは同一オリジンのアセット、WASM、Tauri IPCを許可する。wtermなどが動的に設定するスタイルのためにインラインスタイルを許可する。開発用にはViteのインラインスクリプトとHMR接続を追加する。 [Tauri Capabilities](https://v2.tauri.app/security/capabilities/) [Tauri CSP](https://v2.tauri.app/security/csp/)

ローカルのUIは端末を操作できる信頼済みコードとして扱う。既存の `terminal_create` は `argv` とシェルに渡す `line` を受け付けるため、ファイル・リポジトリ・端末出力から読み込んだ値をコマンドとして自動実行しない。ネイティブ側のライフサイクルログは固定のエラーコードを出力する。coreと設定ワーカーのstderrはアプリログに取り込まない。IPCのエラー本文や端末メタデータを、TypeScriptの永続ログへそのまま保存しない。

`config_eval_poc` はデバッグビルドで、起動環境に `UITERM_ENABLE_CONFIG_POC=1` を設定した場合にリポジトリ同梱のサンプルだけを実行できる。配布ビルドと既定の開発起動では `untrusted_config` を返す。Node.jsは絶対パスのPATHエントリーから探す。ワークスペース内のTypeScript設定の検出、信頼の付与、権限設計はPhase 4で扱う。

## 配布と固定版の更新

| 方式                      | macOS                                                                                    | NixOS                                                             | 保守・容量                                                             |
| ------------------------- | ---------------------------------------------------------------------------------------- | ----------------------------------------------------------------- | ---------------------------------------------------------------------- |
| 外部依存（Phase 1の推奨） | 開発シェルまたは明示した絶対パスで起動。Finderからの起動には将来ランチャー等の導線が必要 | 固定出力のNix derivationをラッパーから参照できる                  | アプリ自体のバンドルを小さく保てる。利用に必要な総容量にはcoreを含める |
| sidecar同梱               | Finderから起動してもアプリとcoreを同じ版で使用できる。署名・公証の対象が増える           | 対象アーキテクチャごとの同梱とNixOS向けランタイム環境の検証が必要 | 対象ごとの取得、ハッシュ検証、署名、更新をリリース工程に追加する       |

当面は外部依存を維持する。配布経路を変更する際は、macOSの単体インストール要件とNixOSパッケージの導線を確認して決める。Tauriのsidecarを採る場合も、WebViewに任意プロセス起動の権限を付与せずRustから起動できる。Node.jsは開発・検証用で、製品の起動依存にもバンドル内容にも加えない。 [Tauri sidecar](https://v2.tauri.app/develop/sidecar/)

`flake.nix` の公式tarball URLとプラットフォーム別SHA-256で配布物を固定する。`HERDR_CORE_BIN` または絶対パスのPATHエントリーを信頼する場合も、起動前の `--version` と接続時の `hello.version` がともに0.1.22であることを確認する。バージョン応答自体は実体の整合性証明ではないため、Nix外の配布では検証済みアーカイブから展開する。更新時は出所・ハッシュとRustの固定版を同じ変更で更新し、Framed Mode、holderの再採用、終了処理を対象OSごとに再検証する。Apple SiliconとLinux 2アーキテクチャをFlakeの対象とする。 [固定版リリース](https://github.com/herdrdev/herdr-core/releases/tag/v0.1.22) [nixpkgsのIntel Mac対応](https://nixos.org/manual/nixpkgs/unstable/release-notes#x86_64-darwin-26.11)

`poc_bench` はCargo exampleに移した。通常のバンドルに含まれる実行ファイルは `uiterm` だけで、監視モードも同じ実行ファイルを使う。

## 検証手順

```sh
nix flake check --no-build --all-systems --no-write-lock-file
nix develop --no-write-lock-file
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml installed_cli_compatibility_in_isolated_pty -- --ignored
cargo run --locked --release --manifest-path src-tauri/Cargo.toml --example poc_bench
pnpm tauri build --bundles app
```

macOSの終了経路を再確認するには、テスト用のHOMEとXDGディレクトリを使って `.app/Contents/MacOS/uiterm` を各試行で直接起動する。最初の端末が作成された後、そのGUIのPIDにだけTERM、INT、KILLを1回ずつ別の試行で送る。監視プロセス、core、holder、シェルと専用ディレクトリが消えるかを確認する。coreのクラッシュと別インスタンスの保持は `tests/supervisor.rs` で再現する。

NixOSでは実機の `pkg-config --modversion webkit2gtk-4.1 gtk+-3.0 libsoup-3.0` を確認し、同じCargoテストとClippyを実行する。`env -u GTK_IM_MODULE GDK_BACKEND=wayland pnpm tauri dev` で起動し、実機で入力・リペイント・終了処理を確認する。NixOS配布ラッパーとFcitx5は [Phase 0の実機申し送り](../poc/validation.md#実機への申し送り) に沿って確認する。

## 検証結果

macOS 15.5（Apple Silicon）でRustの単体テスト、TauriのモックIPCテスト、固定版との結合テスト、Clippy、releaseバンドルを確認した。モックIPCではローカルの `main` が構造化された入力エラーを受け取ることと、別WebView・外部オリジンが拒否されることを確認した。隔離PTYではfish、Neovimと `codex --version` を検証した。通常終了・SIGTERM・SIGINT・SIGKILLの各試行では、リリース版WKWebViewから端末が作成され、監視プロセス・core・holder・シェルと専用ディレクトリが解放された。

`.app` の論理サイズは3,800,676 B（3.62 MiB）、外部coreを含めて約9.92 MiB。Phase 0の4,422,468 Bからアプリ単体で621,792 B小さくなった。`Contents/MacOS` の実行ファイルは `uiterm` だけだった。macOS 11向けのリンク時に14.0向けビルド済みオブジェクトの警告が出たため、実行確認の範囲はmacOS 15.5とする。

移設したrelease exampleでは10端末を作成し、全端末の終了後にliveが0となった。5 MiBの試験は終了マーカーを受信し、転送速度は56.00 MiB/sだった。出力クレジットを止めた状態での制御RPC応答は0.23 msだった。いずれも単一試行で、GUIの描画や長時間負荷の評価には使わない。

隔離したHOME・XDGと同一のPhase 0フロントエンド資材で、Phase 0のコミット `5ea6c3d` とPhase 1のreleaseを測った。ウィンドウ可視化までのウォーム起動中央値（各5回）は266.5 msと199.0 ms。1端末のアイドル時Summary Footprint（各11回）は108.42 MiBと113.53 MiBで、Phase 1の監視プロセスは約2.20 MiBだった。試行によってWebKit GPUのFootprintが大きく変動し、Phase 0の統合報告に記録した測定とはシェルのプロファイルも異なる。起動時点はウィンドウ可視化で、初回プロンプト到達や統合後の長時間性能は未評価。 [Phase 0の性能測定](../poc/integration.md#73-最終性能測定結果今後の開発基準値)

Flakeの全対象の評価は通過した。Linuxはderivationの静的評価までで、実機上のビルド、GTK/WebKitGTK、Wayland、SIGTERM・SIGINT・SIGKILL、Fcitx5は未検証。
