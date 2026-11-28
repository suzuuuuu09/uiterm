# インターフェース

GPUI製品版はRust内で描画・入力を処理し、herdr-coreとはUnixソケット上のJSON-RPC・Framed Modeで通信する。監視プロセスが専用coreを復旧し、「再接続」で同じセッションを新規アタッチして履歴と画面を再同期する。表は比較基準のTauri版の契約を示す。[ADR-0010](adr/0010-evaluate-native-gpui-terminal.md) を参照。

| 境界                        | 契約                                                    |
| --------------------------- | ------------------------------------------------------- |
| GUI → Rust                  | Tauri Commands、端末入力のRaw Body                      |
| Rust → GUI                  | Channels、出力のRaw Binaryフレーム                      |
| Rust ↔ herdr-core           | Unixソケット、JSON-RPC、Framed Mode                     |
| UI・CLI・設定・キーバインド | 共通Action Registry（CLIと製品設定の外部公開はPhase 4） |
| 設定ランタイム              | JSON-RPC over stdio（開発用PoCで検証）                  |

## 移行後も維持する条件

- SessionIdとAttachIdを分離し、切断後は新しいアタッチとスナップショットで再同期する。
- ユーザー入力と自動応答を分類する。未交渉の自動応答をユーザー入力へフォールバックしない。
- 消費した出力にクレジットを返し、バッファ・キューに上限を設ける。
- 外部データは実行時検証する。現行Tauri版でRust所有DTOを変更する間はts-rs生成物と合わせる。
- GUI操作は共通Actionを通し、端末出力は状態ストアに流さない。

GUI・CLI・設定から使う共通Action APIのRustへの移植は次の工程で行う。

## 詳細仕様と実装

- [IPC通信仕様](poc/interface.md)
- [Phase 1 Rust](implementation/phase1-backend.md)
- [Phase 1 TypeScript](implementation/phase1-frontend.md)
- [IPC型](../src/ipc/types.ts)、[生成DTO](../src-tauri/bindings/ipc.ts)、[Action Registry](../src/actions/registry.ts)
