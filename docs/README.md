# ドキュメント

| 文書                               | 内容                         |
| ---------------------------------- | ---------------------------- |
| [architecture.md](architecture.md) | 採用構成、現行実装、責務     |
| [development.md](development.md)   | 開発・検証手順               |
| [interfaces.md](interfaces.md)     | IPC、状態同期、操作APIの境界 |
| [status.md](status.md)             | 開発状況、実機検証、残課題   |
| [ADR](adr/README.md)               | 設計判断と変更履歴           |

## 詳細資料

- 設計：[コンセプト](plan/concept.md)、[MVP](plan/mvp.md)、[アーキテクチャ詳細](plan/architecture.md)、[ロードマップ](plan/roadmap.md)
- 実装：[Phase 1 Rust](implementation/phase1-backend.md)、[Phase 1 TypeScript](implementation/phase1-frontend.md)、[Phase 2 GPUI製品移行](implementation/phase2-gpui.md)、[Phase 2 Tauri版](implementation/phase2-terminal.md)
- PoC：[統合](poc/integration.md)、[通信仕様](poc/interface.md)、[最適化](poc/optimization.md)、[検証](poc/validation.md)、[GPUIターミナル](poc/gpui-terminal.md)

設計資料は採用後の方針を示す。実装・PoCレポートは検証当時の記録として保持する。最新の設計判断はADR、現在の実装と検証状況はstatusを参照する。
