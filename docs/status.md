# 開発状況

2026-11-28時点。mainのPhase 2実装と未マージの調査・修正を区別する。

| Phase | 状態 |
| --- | --- |
| 0 技術検証 | 完了 |
| 1 基盤構築 | 完了 |
| 2 ターミナル | 未完了・実機検証と性能課題あり |
| 3 ワークスペース | 保留 |
| 4 カスタマイズ | 未着手（設定実行方式のPoCあり） |
| 5 AIエージェント管理 | 未着手 |
| 6 MVP | 未完成 |

## Phase 2

mainの基準実装は 5ff26e0。シェル、端末描画、内部設定、PTYのIUTF8暫定対応を実装済み。既存の検証結果は [実装レポート](implementation/phase2-terminal.md) を参照。

開発者からの報告では、リサイズと描画修正をローカルの integration/phase2-fixes（5b5a6c4）に統合した。統合後の自動検証はTypeScript 69件、Rust 25件、隔離CLI統合1件が合格。macOSの隔離検証ではリサイズ、Alternate Screen、Unicode幅等を確認した。これらはmainへのマージを示すものではない。

## 残課題

- Neovimの画面更新とスクロールの操作応答性。WebGL試用では一部Nerd Font表示が退行し、十分な体感改善が得られなかった。
- 断続的な絵文字誤描画・選択時の崩れ。リロードで解消したが原因未特定。
- macism実行遅延の再発。起動・終了への影響を調査する。
- 統合版の実マウス、OSクリップボード、トラックパッド、IME実入力・フォーカス移動の検証。
- NixOS / Hyprland実機、30分の連続利用、Ghosttyとの同一条件比較。

## 次の判断

[ADR-0010](adr/0010-evaluate-native-gpui-terminal.md) のGPUI PoCはPR #4でmainにマージ済み。マージ後のアイドル負荷は [GPUI PoC検証レポート](poc/gpui-terminal.md#43-pr-4-マージ後のアイドル計測) を参照。採用は未確定であり、Phase 3は保留する。[ロードマップ](plan/roadmap.md) と [MVP完成条件](plan/mvp.md#完成条件) は維持する。
