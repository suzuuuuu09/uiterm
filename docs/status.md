# 開発状況

2026-11-28時点。GPUIを正式採用した。[ADR-0010](adr/0010-evaluate-native-gpui-terminal.md) を参照。

| Phase                | 状態                                           |
| -------------------- | ---------------------------------------------- |
| 0 技術検証           | 完了                                           |
| 1 基盤構築           | 完了                                           |
| 2 ターミナル         | 未完了。内部機能の移植と製品版の実機回帰が必要 |
| 3 ワークスペース     | Phase 2完了まで保留                            |
| 4 カスタマイズ       | 未着手。設定ランタイムのPoCあり                |
| 5 AIエージェント管理 | 未着手                                         |
| 6 MVP                | 未完成                                         |

## 実装状況

- GPUIの製品起動経路は `src-gpui` の `uiterm`。端末描画・入力とPTY接続を移行した。結果は [GPUI製品移行](implementation/phase2-gpui.md) を参照。
- Tauri版は回帰検証と性能比較の基準。シェル、描画、内部設定、PTYのIUTF8暫定対応を実装済み。[Phase 2実装・検証レポート](implementation/phase2-terminal.md) を参照。
- GPUI PoCはPR #4でマージ済み。アイドル負荷の調査記録はPR #5、ビルド・テスト用CIはPR #6、PoCの未使用コード整理はPR #7でマージ済み。PoCのソースと計測結果は移行前のコミットで参照できる。
- Tauri版の統合修正 `5b5a6c4` の自動検証とmacOS隔離検証はローカルの報告。mainの実装・検証結果とは区別する。

## 検証状況と次の作業

採用判断に使った性能測定、自動テスト、ユーザーのmacOS実機検証の範囲は [ADR-0010の採用根拠](adr/0010-evaluate-native-gpui-terminal.md#採用根拠) を参照。PoCで得た結果を製品版の合格条件には流用せず、製品移行後に回帰検証する。

次は [Phase 2の移行手順](plan/roadmap.md#phase-2--ターミナル未完了gpui移行) に沿って内部端末設定と共通Action APIを移し、製品版の実機回帰を確認する。NixOS / Hyprland実機検証、アイドル時CPUと表示同期通知、`grid` の上流対応は [ADR-0010の残課題](adr/0010-evaluate-native-gpui-terminal.md#consequences) で管理する。
