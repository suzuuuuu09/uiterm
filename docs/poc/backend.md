---
description: "uiterm Phase 0 バックエンド・開発基盤 技術検証レポート"
---

# uiterm Phase 0 バックエンド・開発基盤 技術検証レポート

> 検証時点の記録。統合後の判定、測定条件の差、および修正後の IPC 仕様は [独立検証レポート](./validation.md) を参照。

本ドキュメントは、uiterm Phase 0 におけるバックエンド（`herdr-core` / Tauri 2 IPC / 外部 Node.js TypeScript 設定ランタイム）および開発基盤（Nix Flakes / pnpm / Vite+）の実実装・実測検証結果をまとめたものである。

IPC 通信仕様については [docs/poc/interface.md](./interface.md) を参照。

---

## 1. 検証環境・測定条件

| 項目           | 値                                                                         |
| -------------- | -------------------------------------------------------------------------- |
| OS / Arch      | macOS 15.5 (Darwin 24.5.0, arm64 / Apple Silicon)                          |
| Nix            | Nix 2.35.6 (`flake.nix` + `flake.lock` 固定)                               |
| Rust / Cargo   | `rustc 1.99.1` / `cargo 1.99.0` (Nix devShell 提供)                        |
| Tauri          | `tauri 2.12.7` / `@tauri-apps/api 2.12.5` / `@tauri-apps/cli 2.12.6`       |
| Node.js / pnpm | `Node.js v24.22.2` (LTS) / `pnpm 12.9.0`                                   |
| Vite+          | `vite-plus 1.8.0`                                                          |
| `herdr-core`   | `v0.1.22` (プロトコル世代 1, OpenRPC 1.3.2, Nix `fetchurl` + SHA-256 固定) |
| 互換性検証 CLI | `fish`, `nvim --clean`, `codex --version`                                  |

すべての `herdr-core` 検証は `/tmp/ut-<prefix>-<pid>-<seq>/` 配下に隔離した `HC_SESSION`・`HC_SOCKET`・`XDG_STATE_HOME`・`XDG_CONFIG_HOME`・`XDG_RUNTIME_DIR` を用いて実行し、ユーザーの既存 Herdr セッション・設定には一切触れていない。

---

## 2. 実装済み PoC コンポーネント

- **開発基盤**: [flake.nix](../../flake.nix), [package.json](../../package.json), [src-tauri/Cargo.toml](../../src-tauri/Cargo.toml), [src-tauri/tauri.conf.json](../../src-tauri/tauri.conf.json)
- **IPC 契約・モック**: [docs/poc/interface.md](./interface.md), [src/ipc/types.ts](../../src/ipc/types.ts), [src/ipc/client.ts](../../src/ipc/client.ts), [src/ipc/mock.ts](../../src/ipc/mock.ts)
- **`herdr-core` Rust アダプター**:
  - [src-tauri/src/herdr/protocol.rs](../../src-tauri/src/herdr/protocol.rs) — Framed Mode バイナリコーデック（kind 0〜6）
  - [src-tauri/src/herdr/process.rs](../../src-tauri/src/herdr/process.rs) — 隔離 `herdr-core` デーモン起動・ヘルスチェック・終了クリーンアップ
  - [src-tauri/src/herdr/client.rs](../../src-tauri/src/herdr/client.rs) — Unix Domain Socket 多重化クライアント（JSON-RPC, Attach ストリーム, Events 購読, フロー制御, 自動応答分離）
  - [src-tauri/src/herdr/types.rs](../../src-tauri/src/herdr/types.rs) — OpenRPC 準拠の型定義
- **Tauri 2 IPC**: [src-tauri/src/commands.rs](../../src-tauri/src/commands.rs) — Raw Binary Channel (`Channel<Response>`) と Raw Binary Input (`Request` / `InvokeBody::Raw`) を用いたゼロ Base64 転送
- **TypeScript 設定ランタイム PoC**: [src-tauri/src/config_runtime.rs](../../src-tauri/src/config_runtime.rs), [poc/config-runtime/worker.mjs](../../poc/config-runtime/worker.mjs), [poc/config-runtime/api.ts](../../poc/config-runtime/api.ts), [poc/config-runtime/sample.config.ts](../../poc/config-runtime/sample.config.ts)
- **統合テスト・ベンチマーク**: [src-tauri/tests/herdr_integration.rs](../../src-tauri/tests/herdr_integration.rs), [src-tauri/src/bin/poc_bench.rs](../../src-tauri/src/bin/poc_bench.rs)

---

## 3. 実測結果

### 3.1 単体・統合テスト・静的解析

- `cargo test -- --nocapture`: **4 / 4 合格**（実行時間 0.69 秒）
  - Framed Mode 全 7 フレーム種別（`DATA`, `CREDIT`, `CLOSE`, `RESIZE`, `FOCUS`, `FACTS`, `AUTOMATIC_INPUT`）のエンコード/デコード往復
  - 隔離 `herdr-core` 起動・Framed Mode ハンドシェイク・機能ネゴシエーション（`automatic_input`, `resize_notifications`）
  - セッション作成・アタッチ・ユーザー入力（`DATA`）・リサイズ（`stty size` で `33 111` 反映確認）
  - 端末自動応答の分離送信（`AUTOMATIC_INPUT` kind 6 で PTY へそのまま到達することを確認）
  - OSC 7501 によるエージェント状態遷移（`state=working` → `state=done` および `turn >= 1` イベント受信確認）
  - デタッチ後の再アタッチによる画面リペイント再同期確認
  - `terminal.kill` 後のセッション解放および `server.stop` による終了確認
  - 外部 Node.js 24 での TypeScript 設定ファイル読み込み・ユーザー定義アクション実行・イベントフック発火確認
- `cargo clippy --all-targets -- -D warnings`: **警告 0 件で通過**

### 3.2 `herdr-core` 性能・互換性・リソース実測（`poc_bench`）

| 検証項目                                                    | 実測値                                       | 判定           |
| ----------------------------------------------------------- | -------------------------------------------- | -------------- |
| `herdr-core` デーモン起動時間（ソケット受付まで）           | **45.68 ms**                                 | 良好           |
| Unix Socket 接続 + Framed `hello` ハンドシェイク            | **0.32 ms**                                  | 良好           |
| アイドル時 `herdr-core` RSS                                 | **5.48 MiB** (5,616 KiB)                     | 非常に軽量     |
| `fish -i` 起動・コマンド実行・終了                          | 成功 (`ok=true`)                             | 合格           |
| `nvim --clean` PTY 起動・`:q!` 正常終了                     | 成功 (`exit_code=0`, 12,506 bytes 受信)      | 合格           |
| `codex --version` PTY 実行                                  | 成功 (`ok=true`)                             | 合格           |
| 10 端末同時作成・アタッチ・エコー往復                       | **76.67 ms**（全 10 端末成功）               | 良好           |
| 10 端末同時稼働時 RSS (`herdr-core` / `holder`)             | **14.22 MiB** / **7.47 MiB**（計 21.69 MiB） | 軽量           |
| 10 端末 kill 後（`live=0`）RSS (`herdr-core` / `holder`)    | **10.22 MiB** / **7.00 MiB**                 | 解放確認       |
| 5 MiB バースト出力（5,308,506 bytes, CREDIT 32 KiB 単位）   | **87.09 ms** / **58.13 MiB/s**               | 十分高速       |
| 5 MiB バースト直後 RSS (`herdr-core` / `holder`)            | **11.64 MiB** / **9.05 MiB**                 | メモリ膨張なし |
| ストール中ビューア（6 MiB 出力・CREDIT 0）存在時の Ch.0 RPC | **0.38 ms**（ブロックなし）                  | 隔離確認       |
| `server.stop` 後の残存プロセス（`herdr-core` / `holder`）   | **0 件** (`false` / `false`)                 | リークなし     |

### 3.3 TypeScript 設定ランタイム（外部 Node.js 24 + JSON-RPC over stdio）

Node.js 24 LTS のネイティブ TypeScript 型除去機能（`import("./sample.config.ts")`）により、**バンドラ・トランスパイラ（`tsx` / `ts-node` / `esbuild`）依存ゼロ** で `.ts` 設定ファイルの読み込み・型付きヘルパー関数・ユーザー定義関数・非同期イベントフックを実行できた。

| 実行回 | コールドスタート時間（`ping` 応答まで） | 設定ロード + Action + Hook + Metrics 合計 RPC 時間 | Node.js ワーカー RSS |
| ------ | --------------------------------------- | -------------------------------------------------- | -------------------- |
| Run 1  | **50.47 ms**                            | **23.18 ms**                                       | **36.05 MiB**        |
| Run 2  | **34.01 ms**                            | **21.80 ms**                                       | **34.60 MiB**        |
| Run 3  | **32.34 ms**                            | **21.33 ms**                                       | **33.44 MiB**        |

- **評価**: 起動時 1 回だけ設定を読み込んでワーカーを終了する方式なら常駐メモリ消費は **0 MiB**。イベントフックやカスタムアクション用に常駐させても約 **33〜36 MiB** かつ 1 回あたりの RPC は数 ms オーダーで実用十分。

### 3.4 ディスク容量・依存関係容量

| 対象                                                             | サイズ      | 備考                                                                                  |
| ---------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------- |
| `herdr-core` v0.1.22 バイナリ（macOS arm64）                     | **6.3 MiB** | tarball 5.40 MB。Linux x86_64 静的ビルドは tarball 3.30 MB、Linux aarch64 は 3.04 MB  |
| `src-tauri/target/release/uiterm`（デフォルト release）          | **11 MiB**  | LTO・strip 未適用。Phase 1 以降で `lto = true, strip = true` 適用によりさらに削減可能 |
| `node_modules`（開発依存 + フロントエンド検証用 4 種描画器含む） | **343 MiB** | 開発時のみ。アプリバンドルには同梱されない                                            |
| `src-tauri/target`（debug + release キャッシュ）                 | **3.7 GiB** | 開発ビルドキャッシュのみ                                                              |

---

## 4. 検証で発見した問題と解決策・代替案

### 問題 1: 多重化ソケットリーダでのヘッドオブライン（HOL）ブロッキング【重要】

- **事象**: 初期実装で `herdr-core` の単一ソケットリーダタスクから各アタッチストリームへ有界キュー（上限 64 フレーム）で `tx.send(item).await` を行ったところ、1 つのビューアがストール（CREDIT 未返却）した際にソケットリーダ全体がブロックし、チャンネル 0 の `server.status` RPC まで 120 秒以上ハングした。
- **原因**: `herdr-core` の Framed Mode は単一 Unix Socket 上にチャンネル 0（JSON-RPC）と複数端末ストリームを多重化する。フレーム単位の小さな有界チャネルでリーダ側が `.await` すると、256 KiB のクレジット上限に達する前にリーダが詰まり、`herdr-core` 側のストリーム隔離が破綻する。
- **解決策**: ソケットリーダからの配送はノンブロッキング（`UnboundedSender::send`）とし、ストリーム単位のメモリ上限管理は `herdr-core` の **256 KiB クレジットウィンドウ** に委ねる設計へ修正。フロントエンドが `terminal_ack` を返さない限り `herdr-core` が追加フレーム送信を止めるため、メモリは厳密に有界かつストール中の Ch.0 RPC 応答は **0.38 ms** まで改善した。

### 問題 2: `events.subscribe` チャンネルでの複数 JSON-RPC 通知結合

- **事象**: OSC 7501 で `working` → `done` へ遷移した際、`herdr-core` が `status` イベントと `turn` イベントを単一の `DATA` フレーム内に改行区切りで連結して送信したため、フレーム単位の `serde_json::from_slice` がパース失敗し両イベントが欠落した。
- **解決策**: イベント購読チャンネルごとにバイトバッファ（`event_buffers`）を持ち、`\n` 区切りで 1 行ずつ JSON-RPC 通知を取り出すパーサを実装して解消。

### 問題 3: `terminal.attach` / `events.subscribe` 応答直後の初回フレーム取りこぼし

- **事象**: `terminal.attach` のチャンネル 0 応答を受信してから `attach_streams` マップへ登録するまでの僅かな非同期スケジューリング隙間で、ソケットリーダが初回リペイント `DATA` フレームを受信・破棄してしまうレースコンディションが発生し、再アタッチ時のリペイントが空になった。
- **解決策**: 未登録チャンネル宛てのフレームを一時保持する `early_frames` バッファを導入し、登録完了直後に到着順でドレインすることで完全に解消。

### 問題 4: OSC 7501 の正確なプロトコル仕様と `Terminal.status` 型

- **発見**: `herdr-core` の実装（`crates/herdr-core/src/status.rs` および `hc.openrpc.json`）を調査したところ、OSC 7501 は `\x1b]7501;state=working:app=codex\x1b\\`（コロン区切り key=value）であり、`Terminal.status` は文字列ではなく構造体 `StatusRecord { state, kind?, progress?, app?, title?, msg? }` であった。ドキュメントおよび Rust / TypeScript 型定義を実仕様に合わせて統一した。
- **注意点**: `herdr-core` は OSC 7501 を明示的に出力するプログラムの状態のみを記録し、画面テキストからの推測は行わない。Codex 等の既存エージェントが OSC 7501 を直接出力しない場合は、Phase 5 でエージェント Hooks から OSC 7501 または `doc.set` 経由で状態連携する設計が必要。

### 問題 5: Nix Flakes `path:.` 評価時の競合と Vite+ / Tauri 連携

- **事象**: `flake.nix` が Git 未追跡の状態で `nix develop path:.` を並列実行すると、`src-tauri/target/` 内のビルド中一時ファイルを Nix がコピーしようとして失敗するケースを確認した。
- **解決策**: `flake.nix` と `flake.lock` を Git 追跡対象に含めれば、Nix は追跡ファイルのみをスナップショットするため `target/` や `node_modules/` を一切コピーしない。
- **Linux / NixOS 評価**: `nix eval path:.#devShells.x86_64-linux.default.name` は警告・エラーなしで評価完了した。

---

## 5. 採用技術の継続・変更判断と根拠

| 技術                                       | 判断                       | 根拠                                                                                                                                                                                                                                                                                                                     |
| ------------------------------------------ | -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Tauri 2**                                | **継続**                   | `tauri 2.12.7` の `Channel<Response>` + `Request` (`InvokeBody::Raw`) により Base64 なしのバイナリ双方向 IPC が簡潔に実装可能。リリースバイナリも 11 MiB（未 strip）と非常に軽量。                                                                                                                                       |
| **`herdr-core` (v0.1.22)**                 | **継続**                   | アイドル 5.5 MiB、10 端末 21.7 MiB、58 MiB/s の高スループット、OpenRPC による厳格な後方互換保証、`AUTOMATIC_INPUT` (kind 6) による端末自動応答のネイティブ分離、256 KiB クレジットフロー制御と 4 MiB 超過時リペイント再同期、OSC 7501 状態管理まで要件をすべて満たす。独自 PTY 管理（`portable-pty` 等）への置換は不要。 |
| **Tokio / Serde**                          | **継続**                   | 非同期 Unix Socket 多重化と JSON-RPC 処理に過不足なく適合。                                                                                                                                                                                                                                                              |
| **外部 Node.js 24 LTS（TypeScript 設定）** | **継続**                   | ネイティブ型除去により外部トランスパイラ不要で `.ts` 設定・関数・非同期フックを実行可能。起動 32〜50 ms・RSS 約 35 MiB でアプリへのランタイム同梱も不要。なお、設定のみ静的に読む場面と常駐フックが必要な場面でオンデマンド起動を使い分ける設計が望ましい。                                                              |
| **Nix Flakes**                             | **継続**                   | `herdr-core` v0.1.22 を SHA-256 固定で取り込み、macOS（arm64/x86_64）と NixOS（x86_64/arm64, Wayland/WebKitGTK）の再現可能な開発環境を単一 `flake.nix` で構築できた。                                                                                                                                                    |
| **Vite+ (`vite-plus`)**                    | **条件付き継続**           | `vite-plus 1.8.0` は pnpm と正常動作するが、`vp` ラッパーがパッケージマネージャを透過管理する仕様上、Tauri CLI（`beforeDevCommand` / `beforeBuildCommand`）および Nix 環境では `pnpm dev` / `pnpm build` 経由で呼び出す構成を明示維持する必要がある。                                                                    |
| **ts-rs**                                  | **Phase 1 以降で導入検討** | Phase 0 では型数が少なく手動同期で十分だったが、Phase 1 で IPC コマンドが増える段階で `ts-rs` による Rust → TypeScript 自動型生成を導入すると型ドリフトを防げる。                                                                                                                                                        |

---

## 6. 未検証項目と再現手順

以下の項目は本環境（macOS arm64、バックエンド単体検証）では未実測であり、成功扱いとしない。

1. **NixOS / Hyprland（ネイティブ Wayland + WebKitGTK + Fcitx5）実機でのビルド・実行**
   - `flake.nix` の `x86_64-linux` 評価までは確認済みだが、Linux 実機での `cargo build`・Tauri ウィンドウ表示・Wayland IME 連携は未検証。
2. **Tauri 実ウィンドウ（WKWebView / WebKitGTK）上でのフロントエンド + バックエンド結合時の E2E 遅延・CPU/メモリ**
   - バックエンド単体（Rust ↔ `herdr-core`）でのスループット・RSS は実測済みだが、`wterm` + `libghostty` WASM との結合による VT 二重動作の合計負荷は、フロントエンド PoC との結合フェーズで実測が必要。
3. **Codex のインタラクティブ TUI セッション長時間稼働**
   - 本 PoC では非破壊制約のため `codex --version` の PTY 実行確認に留めている。

### 再現手順

```sh
# 1. Nix Flakes 開発環境で Rust 単体・統合テストを実行
nix --extra-experimental-features "nix-command flakes" develop path:. --command \
  sh -c 'cd src-tauri && cargo test -- --nocapture'

# 2. 性能・互換性・フロー制御・TS 設定ランタイムのベンチマークを実行
nix --extra-experimental-features "nix-command flakes" develop path:. --command \
  sh -c 'cd src-tauri && cargo run --bin poc_bench'

# 3. 静的解析およびリリースビルド
nix --extra-experimental-features "nix-command flakes" develop path:. --command \
  sh -c 'cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo build --release --bin uiterm'
```
