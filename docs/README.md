# ドキュメント

| 文書 | 内容 |
| --- | --- |
| [architecture.md](architecture.md) | 現行構成、責務、設計上の制約 |
| [development.md](development.md) | 開発・検証手順 |
| [interfaces.md](interfaces.md) | IPC、状態同期、操作APIの境界 |
| [status.md](status.md) | 開発状況、実機検証、残課題 |
| [ADR](adr/README.md) | 設計判断と変更履歴 |

## 詳細資料

- 設計：[コンセプト](plan/concept.md)、[MVP](plan/mvp.md)、[アーキテクチャ詳細](plan/architecture.md)、[ロードマップ](plan/roadmap.md)
- 実装：[Phase 1 Rust](implementation/phase1-backend.md)、[Phase 1 TypeScript](implementation/phase1-frontend.md)、[Phase 2 ターミナル](implementation/phase2-terminal.md)
- PoC：[統合](poc/integration.md)、[通信仕様](poc/interface.md)、[最適化](poc/optimization.md)、[検証](poc/validation.md)

詳細文書は検証当時の記録として保持する。最新の設計変更案はADR、未マージの実装との区別はstatusを参照する。
