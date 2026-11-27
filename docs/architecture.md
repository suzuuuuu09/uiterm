# アーキテクチャ

## 現行構成

```text
React UI / Action Registry / Zustand Vanilla
          │
TerminalSessionManager → wterm + Ghostty WASM
          │ Tauri Commands / Raw Channels
Rust adapter / process supervisor
          │ Unix socket / JSON-RPC / Framed Mode
herdr-core → PTY / shell / CLI
```

| 領域 | 責務 |
| --- | --- |
| src/app, src/state, src/actions | アプリ状態、共通操作API、レイアウト |
| src/terminal | 入出力、VT描画、アタッチ、フォーカス |
| src/ipc, src/validation | IPC契約と外部入力検証 |
| src-tauri/src | ネイティブ処理、core接続、監視、DTO |
| src/i18n | 日本語・英語の翻訳キーと書式 |
| src/poc | PoC専用診断 |

herdr-coreがセッション状態の正、uitermがWindow → Workspace → Tab → Paneのレイアウトと選択状態を所有する。端末出力はストアを経由させない。プロセス監視、クレジット、再同期、機密情報の非記録を維持する。

外部Node.jsによるTypeScript設定はPoC検証済みで、製品機能は未完成。内蔵ブラウザはPost-MVPで、terminal-browserは候補外とする。

## 構成の再評価

Rust + GPUI + ネイティブlibghostty-vt + herdr-coreを第一候補として検証する。採用決定前は現行構成を維持する。[ADR-0010](adr/0010-evaluate-native-gpui-terminal.md) 参照。

詳細：[設計資料](plan/architecture.md)、[ADR一覧](adr/README.md)。
