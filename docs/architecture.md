# アーキテクチャ

## 採用した製品構成

```text
GPUI（UI・端末描画）
  ├─ 共通Action API・レイアウト状態（Rust）
  └─ ネイティブ版libghostty-vt ↔ Rustアダプター ↔ herdr-core ↔ PTY / CLI
```

Rustを主要言語とする。herdr-coreがセッション状態の正で、uitermはWindow → Workspace → Tab → Paneのレイアウトと選択状態を所有する。設定・拡張は外部Node.jsでTypeScriptを実行し、JSON-RPC over stdioを介して接続する。開発環境はNixを使う。

移行後の構成と引き継ぐ契約は [ADR-0010](adr/0010-evaluate-native-gpui-terminal.md) と [設計資料](plan/architecture.md) を参照。

## 現行実装

製品の `uiterm` は `src-gpui` からビルドする。GPUIがウィンドウ・描画・入力を管理し、ネイティブ版libghostty-vtがVT状態を保持する。専用の監視プロセスは `native/herdr` にあり、Tauri版と共用する。GPUIのベンチマークと診断UIは `src-gpui/src/diagnostics.rs` にある。共通Action API・内部設定のRust移植は次の工程で行う。

比較基準のTauri版は以下の構成で引き続きビルドできる。

```text
React UI / Action Registry / Zustand Vanilla
          │
TerminalSessionManager → wterm + Ghostty WASM
          │ Tauri Commands / Raw Channels
Rust adapter / process supervisor
          │ Unix socket / JSON-RPC / Framed Mode
herdr-core → PTY / shell / CLI
```

| 領域                            | 責務                                 |
| ------------------------------- | ------------------------------------ |
| src/app, src/state, src/actions | アプリ状態、共通操作API、レイアウト  |
| src/terminal                    | 入出力、VT描画、アタッチ、フォーカス |
| src/ipc, src/validation         | IPC契約と外部入力検証                |
| src-tauri/src                   | ネイティブ処理、core接続、DTO        |
| src/i18n                        | 日本語・英語の翻訳キーと書式         |
| src/poc                         | PoC専用診断                          |

現行Tauri版の境界は [通信・操作契約](interfaces.md)、GPUIへの移行状況は [開発状況](status.md) を参照。外部Node.jsによるTypeScript設定は開発用PoCで方式を検証済みで、製品機能は未完成。
