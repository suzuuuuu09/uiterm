---
description: "uiterm Phase 0 バックエンド・フロントエンド間 Tauri IPC 通信仕様"
---

# uiterm Phase 0 IPC 通信仕様

本ドキュメントは、Rust バックエンド（Tauri 2 + `herdr-core` アダプター）と TypeScript フロントエンド間の通信仕様を定義する。
TypeScript 側の型定義・実装は [src/ipc/types.ts](../../src/ipc/types.ts)、[src/ipc/client.ts](../../src/ipc/client.ts)、モック実装は [src/ipc/mock.ts](../../src/ipc/mock.ts) に配置している。

---

## 1. 識別子

| 識別子 | 型 | 説明 |
| --- | --- | --- |
| `SessionId` | `string` | `herdr-core` が採番する不変の端末アドレス（例: `"t1"`, `"t2"`）。プロセス終了後も一意。 |
| `AttachId` | `number` (`u32`) | `terminal_attach` で割り当てられる Framed Mode のストリームチャンネル ID。`terminal_write` / `terminal_resize` / `terminal_focus` / `terminal_facts` / `terminal_ack` / `terminal_detach` の対象指定に使用する。 |
| `subscriptionId` | `number` (`u32`) | `events_subscribe` で割り当てられるイベント購読チャンネル ID。 |

1 つの `SessionId` に対して複数回 `terminal_attach`（再接続や `observe: true` での監視）を行えるため、セッション識別子とアタッチストリーム識別子を分離している。

---

## 2. Tauri Commands 一覧

すべてのエラーは以下の JSON 構造（`IpcError`）として reject される。

```ts
export interface IpcError {
  code: string;        // 例: "not_found", "unavailable", "core_disconnected", "invalid_args", "io_error"
  message: string;
  hint?: string | null;
}
```

### 2.1 コア接続・状態

| コマンド名 | 引数 | 戻り値 | 説明 |
| --- | --- | --- | --- |
| `core_status` | なし | `CoreStatus` | 起動中 `herdr-core` のバージョン・PID・稼働時間・対応機能（`automaticInputSupported`, `resizeNotificationsSupported`）を返す。 |
| `core_reconnect` | なし | `CoreStatus` | Unix Domain Socket を再接続し Framed Mode ハンドシェイクをやり直す。 |

### 2.2 ターミナル管理

| コマンド名 | 引数 | 戻り値 | 説明 |
| --- | --- | --- | --- |
| `terminal_create` | `{ params: TerminalCreateParams }` | `TerminalInfo` | `herdr-core` の `terminal.run` を呼び出して新規セッションを作成する。`argv` と `line` を省略するとデフォルトシェルを起動する。 |
| `terminal_list` | `{ all: boolean }` | `TerminalInfo[]` | 実行中（`all: true` なら終了済み含む）のセッション一覧を返す。 |
| `terminal_attach` | `{ params: TerminalAttachParams, onOutput: Channel<ArrayBuffer> }` | `TerminalAttachResult` | 指定 `sessionId` にアタッチし、`onOutput` チャンネルでバイナリストリームの配信を開始する。 |
| `terminal_detach` | `{ attachId: AttachId }` | `void` | `herdr-core` へ `CLOSE` フレームを送り、アタッチを解除する（シェルプロセスは維持される）。 |
| `terminal_kill` | `{ sessionId: SessionId }` | `void` | `herdr-core` の `terminal.kill` を呼び出してプロセスを終了する。 |

### 2.3 ターミナル入出力・制御

| コマンド名 | 引数 | 戻り値 | 説明 |
| --- | --- | --- | --- |
| `terminal_write` | **Raw Binary Body** (`Uint8Array`) + Headers | `void` | 端末への入力を送信する（詳細は 4 章）。 |
| `terminal_resize` | `{ attachId: AttachId, cols: number, rows: number }` | `void` | `herdr-core` へ `RESIZE` フレーム（kind 3）を送信する。 |
| `terminal_focus` | `{ attachId: AttachId, focused: boolean }` | `void` | `herdr-core` へ `FOCUS` フレーム（kind 4）を送信する。アクティブビューア切替・フォーカスイベント送信に使用。 |
| `terminal_facts` | `{ attachId: AttachId, display: DisplayFacts }` | `void` | `herdr-core` へ `FACTS` フレーム（kind 5）を送信する（前景色・背景色・カーソル色・セルサイズ）。 |
| `terminal_ack` | `{ attachId: AttachId, bytes: number }` | `void` | フロントエンドが消費したバイト数を `herdr-core` へ `CREDIT` フレーム（kind 1）として返す。 |

### 2.4 イベント購読

| コマンド名 | 引数 | 戻り値 | 説明 |
| --- | --- | --- | --- |
| `events_subscribe` | `{ onEvent: Channel<CoreEvent> }` | `EventsSubscribeResult` | `herdr-core` の `events.subscribe` を開始し、初期スナップショット（`seq`, `terminals`）を返しつつ差分イベントを配信する。 |
| `events_unsubscribe` | `{ subscriptionId: number }` | `void` | イベント購読チャンネルを閉じる。 |

### 2.5 TypeScript 設定ランタイム検証用

| コマンド名 | 引数 | 戻り値 | 説明 |
| --- | --- | --- | --- |
| `config_eval_poc` | `{ params: ConfigEvalParams }` | `ConfigEvalResult` | 外部 Node.js プロセスを起動し、JSON-RPC over stdio で TypeScript 設定ファイル・ユーザー定義アクション・イベントフックを評価する。 |

---

## 3. バイナリストリーム仕様（Backend → Frontend）

`terminal_attach` に渡す `onOutput: Channel<ArrayBuffer>` は、Tauri v2 の `tauri::ipc::Response`（`InvokeResponseBody::Raw`）を用いて **JSON / Base64 シリアライズなし** で生バイト列を転送する。

すべてのメッセージは先頭 1 バイトのフレームタグを持つ：

```text
+---------------+----------------------------------------+
| tag (1 byte)  | payload (0 .. N bytes)                 |
+---------------+----------------------------------------+
```

| `tag` | 定数名 | ペイロード形式 | 説明 |
| --- | --- | --- | --- |
| `0x00` | `OUTPUT_FRAME_DATA` | 生 VT / PTY 出力バイト列 | 初回は `herdr-core` による現在画面の再描画（+ オプションの履歴 + OSC 7501 状態 + タイトル + `CSI ?9001h/l`）、以降はライブ出力。`wterm` へそのまま流し込む。 |
| `0x01` | `OUTPUT_FRAME_RESIZE` | `[cols: u16 LE][rows: u16 LE]` (4 bytes) | `resizeNotifications: true` 指定時に送信される。後続の `DATA` を処理する前に描画グリッドをリサイズすること。 |
| `0x02` | `OUTPUT_FRAME_CLOSE` | UTF-8 JSON（省略可） | ストリーム終了通知。例: `{"reason":"exited","exit_code":0,"signal":null}`。 |

フロントエンドは `decodeOutputFrame(buffer: ArrayBuffer)`（[src/ipc/types.ts](../../src/ipc/types.ts)）でデコードできる。

---

## 4. ユーザー入力と端末の自動応答の分離（Frontend → Backend）

`terminal_write` は Tauri v2 の Raw Body IPC（`tauri::ipc::Request`）を使用し、JSON 配列化によるオーバーヘッドを避ける。

- **HTTP-like Headers**:
  - `x-uiterm-attach-id`: `<AttachId>`（必須、u32 文字列）
  - `x-uiterm-input-kind`: `"user"` または `"automatic"`（省略時は `"user"`）
- **Body**: 生バイト列（`Uint8Array`）

### バックエンドの転送ルール

1. `kind === "user"` の場合：常に `herdr-core` の **DATA フレーム（kind 0）** で送信する。
2. `kind === "automatic"` の場合：
   - `hello.features` に `"automatic_input"` が含まれ、かつ `terminal.attach` 応答の `automatic_input === true` の場合のみ、`herdr-core` の **AUTOMATIC_INPUT フレーム（kind 6）** で送信する。
   - 非対応の `herdr-core` に対しては仕様に従い **DATA フレーム（kind 0）** へフォールバックする（非対応コアへ kind 6 を送ると入力が破棄されるため）。
3. **フロントエンド側の責務**：
   - `wterm` / `libghostty` が端末クエリ（DA1/DA2、OSC カラー問い合わせ、mode 2048、XTGETTCAP 等）への自動応答を生成するコールバックからは必ず `kind: "automatic"` を指定して `terminalWrite` を呼ぶこと。バイト内容からの推測は行わない。

---

## 5. フロー制御・過剰バッファリング防止

1. `herdr-core` のアタッチストリームは初期クレジット **256 KiB**（`initialCreditBytes: 262144`）で開始する。
2. Rust バックエンドの多重化ソケットリーダは、ストリーム単位でブロッキングしないノンブロッキング配送を行い、ヘッドオブライン（HOL）ブロッキング（1 つの遅延端末がチャンネル 0 の RPC や他端末を止める現象）を防ぐ。メモリ上限は `herdr-core` 側の 256 KiB クレジットウィンドウによって厳密に制御される（フロントエンドが `terminalAck` を返すまで `herdr-core` が追加送信を停止するため）。
3. フロントエンドは `OUTPUT_FRAME_DATA` を `wterm` へ適用したあと、消費済みバイト数をまとめて `terminalAck(attachId, bytes)` で通知する（目安: 16 KiB 以上または 1 フレームごと）。
4. バックエンドは受け取ったバイト数を `herdr-core` の **CREDIT フレーム（kind 1）** として返送する。
5. フロントエンドが **4 MiB 以上遅延** した場合、`herdr-core` が自動的に未読バックログを破棄し、最新画面のフルリペイントを送信して再同期する。

---

## 6. イベントストリーム仕様（`events_subscribe`）

`CoreEvent` は `type` を判別子とするタグ付きユニオンで配信される。主なイベント：

- ライフサイクル: `created`, `lost`, `restarted`, `finished`, `removed`
- 端末メタデータ: `title`, `size`, `foreground`, `labels`
- AI エージェント状態（OSC 7501）: `status`（`{ status, records, reported }`）, `turn`（`{ turn, state }`）
- 接続監視・再同期: `heartbeat`（15 秒間隔）, `missed`（遅延時の最新 `terminals` スナップショット）, `disconnected`

`missed` を受信した場合、フロントエンドはローカルの端末一覧を `event.terminals` で置き換え、`event.seq` 以下の古いイベントを破棄する。

---

## 7. リサイズ・切断・再接続

- **リサイズ**: ウィンドウ・ペインのサイズ変更時は `terminalResize(attachId, cols, rows)` を呼ぶ。`herdr-core` では最後にアクティブになったビューア（アタッチ・入力・リサイズ・フォーカス取得）のサイズが採用される。
- **切断**: `herdr-core` との Unix Socket が切断されると、全アタッチストリームへ `OUTPUT_FRAME_CLOSE { reason: "disconnected" }`、全イベント購読へ `{ type: "disconnected", reason }` が配信される。
- **再接続**: フロントエンドは `coreReconnect()` を呼んだ後、`eventsSubscribe()` と各タブの `terminalAttach()` を再実行する。`terminalAttach()` は `herdr-core` から現在画面のフルリペイントを受け取るため、表示状態は自動的に再同期される。

---

## 8. フロントエンド単独開発用モックの利用方法

フロントエンド担当者は Tauri / `herdr-core` を起動せずにブラウザ単体で開発・検証できる。

```ts
import { isTauriRuntime, TauriIpcClient } from "./ipc/client";
import { MockIpcClient } from "./ipc/mock";
import type { UitermBackendClient } from "./ipc/types";

export const backend: UitermBackendClient = isTauriRuntime()
  ? new TauriIpcClient()
  : new MockIpcClient();
```

`MockIpcClient` は `recordedInputs` に `{ attachId, kind, bytes }` を記録するため、ユーザー入力（`"user"`）と自動応答（`"automatic"`）の分離テストにもそのまま利用できる。
