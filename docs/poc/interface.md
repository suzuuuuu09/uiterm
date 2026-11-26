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
| `AttachId` | `number` (`u32`) | `terminal_attach` で割り当てられる uiterm のプロセス内で一意なストリーム ID。Rust が接続ごとの Framed Mode チャンネルに変換する。`terminal_write` / `terminal_resize` / `terminal_focus` / `terminal_facts` / `terminal_ack` / `terminal_detach` の対象指定に使用する。 |
| `subscriptionId` | `number` (`u32`) | `events_subscribe` で割り当てられるイベント購読 ID。アタッチと同様、Rust が接続ごとのチャンネルに変換する。 |

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
| `core_reconnect` | なし | `CoreStatus` | 稼働中の接続を再利用する。コアが稼働したまま接続が切断された場合、同じ隔離コアに再接続して Framed Mode ハンドシェイクをやり直す。コア停止時はエラーを返し、自動再起動は行わない。 |

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
| `events_subscribe` | `{ onEvent: Channel<{ subscriptionId: number, events: CoreEvent[], bytes: number }> }` | `EventsSubscribeResult` | `herdr-core` の `events.subscribe` を開始し、初期スナップショット（`seq`, `terminals`）を返しつつ差分イベントを配信する。 |
| `events_ack` | `{ subscriptionId: number, bytes: number }` | `void` | クライアントがイベントを受け取った後、JSON の分割分も含めた受信バイト数を CREDIT で返す。 |
| `events_unsubscribe` | `{ subscriptionId: number }` | `void` | イベント購読チャンネルを閉じる。 |

### 2.5 TypeScript 設定ランタイム検証用

| コマンド名 | 引数 | 戻り値 | 説明 |
| --- | --- | --- | --- |
| `config_eval_poc` | `{ params: ConfigEvalParams }` | `ConfigEvalResult` | 外部 Node.js プロセスを起動し、JSON-RPC over stdio で リポジトリ内の PoC サンプル設定を評価する。WebView から渡された任意パスは拒否する。 |

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
   - 自動入力が未交渉の経路では `unsupported` を返す。旧クライアント向け仕様の DATA フォールバックではユーザー操作として扱われるため、uiterm の PoC では自動応答を送信しない。
3. **フロントエンド側の責務**：
   - `wterm` / `libghostty` が端末クエリ（DA1/DA2、OSC カラー問い合わせ、mode 2048、XTGETTCAP 等）への自動応答を生成するコールバックからは必ず `kind: "automatic"` を指定して `terminalWrite` を呼ぶこと。バイト内容からの推測は行わない。

---

## 5. フロー制御・過剰バッファリング防止

1. 端末とイベントの各ストリームは初期 CREDIT 256 KiB で開始する。Rust は受信済みかつ未 ACK のバイト数を追跡する。範囲を超える ACK を拒否し、受信側が CREDIT ウィンドウを超えた場合は接続を閉じる。
2. Rust は購読を RPC 応答の処理中に登録する。フレームを待たずにキューへ配信し、満杯なら接続を閉じて再同期を要求する。端末キューは 4,096 件、イベントキューは 256 件で、CLOSE 用に各1枠を追加する。イベント購読応答前の保留は接続全体で 256 件・256 KiB とする。
3. wterm の同期 VT 処理が戻った後、消費した DATA を 16 KiB 単位でまとめて送る。RESIZE と CLOSE は ACK の対象外。イベントは WebView で受け取ったバッチごとに ACK する。固定版の上流は 4 MiB 超の未送信バックログを破棄して再同期を行う。
4. uiterm は接続あたり最大 128 ストリーム、保留 RPC と送信キューを各 64 件、送信ペイロードを 1 MiB に制限する。フレームの待機と RPC 応答にタイムアウトを設ける。フロントエンドはアタッチごとに入力を直列送信し、送信中と待機中の入力を合計 1 MiB、待機を 256 件までに制限する。長大な貼り付けの分割は今後の課題とする。

---

## 6. イベントストリーム仕様（`events_subscribe`）

`CoreEvent` は `type` を判別子とするタグ付きユニオンで配信される。主なイベント：

- ライフサイクル: `created`, `lost`, `restarted`, `finished`, `removed`
- 端末メタデータ: `title`, `size`, `foreground`, `labels`
- AI エージェント状態（OSC 7501）: `status`（`{ status, records, reported }`）, `turn`（`{ turn, state }`）
- 接続監視・再同期: `heartbeat`（15 秒間隔）, `missed`（遅延時の最新 `terminals` スナップショット）, `disconnected`

Phase 0 の UI は `missed` を診断ログに記録する。イベントを一覧の更新に使用する Phase 1 では、`event.terminals` の適用と `event.seq` 以下のイベントの破棄が必要になる。

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
