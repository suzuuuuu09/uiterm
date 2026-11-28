# インターフェース

現行Tauri版の契約への入口。詳細仕様と実装を参照し、ここで新規プロトコルを定義しない。

| 境界                        | 契約                                                    |
| --------------------------- | ------------------------------------------------------- |
| GUI → Rust                  | Tauri Commands、端末入力のRaw Body                      |
| Rust → GUI                  | Channels、出力のRaw Binaryフレーム                      |
| Rust ↔ herdr-core           | Unixソケット、JSON-RPC、Framed Mode                     |
| UI・CLI・設定・キーバインド | 共通Action Registry（CLIと製品設定の外部公開はPhase 4） |
| 設定ランタイム              | JSON-RPC over stdio（開発用PoCで検証）                  |

## 不変条件

- SessionIdとAttachIdを分離し、切断後は新しいアタッチとスナップショットで再同期する。
- ユーザー入力と自動応答を分類する。未交渉の自動応答をユーザー入力へフォールバックしない。
- 消費した出力にクレジットを返し、バッファ・キューに上限を設ける。
- Rust所有DTOの変更はts-rs生成物と合わせる。外部データは実行時検証する。
- GUI操作は共通アクションを通し、端末出力は状態ストアに流さない。

## 詳細仕様と実装

- [IPC通信仕様](poc/interface.md)
- [Phase 1 Rust](implementation/phase1-backend.md)
- [Phase 1 TypeScript](implementation/phase1-frontend.md)
- [IPC型](../src/ipc/types.ts)、[生成DTO](../src-tauri/bindings/ipc.ts)、[Action Registry](../src/actions/registry.ts)
