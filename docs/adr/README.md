# Architecture Decision Records

設計判断の背景、選択、代替案、影響を記録する。ADR-0001〜0009は現行の設計資料・実装記録から再構成したもので、日付は記録作成日であり、実際の決定日を推定したものではない。

| ADR  | 判断                                                                                   | Status   |
| ---- | -------------------------------------------------------------------------------------- | -------- |
| 0001 | [TypeScript・Tauri・Rustの採用](0001-adopt-typescript-tauri-rust.md)                   | Accepted |
| 0002 | [herdr-coreによるセッション管理](0002-adopt-herdr-core.md)                             | Accepted |
| 0003 | [wterm・Ghostty WASMによる端末描画](0003-adopt-wterm-ghostty-wasm.md)                  | Accepted |
| 0004 | [Zustand Vanilla・Action Registry](0004-zustand-action-registry.md)                    | Accepted |
| 0005 | [IPC・プロセス隔離・セキュリティ](0005-ipc-process-isolation-security.md)              | Accepted |
| 0006 | [外部Node.jsによるTypeScript設定](0006-external-node-typescript-config.md)             | Accepted |
| 0007 | [Nix・pnpm・Vite+の採用](0007-nix-pnpm-vite-plus.md)                                   | Accepted |
| 0008 | [PTYのIUTF8暫定対応](0008-pty-iutf8-workaround.md)                                     | Accepted |
| 0009 | [TypeScript辞書による国際化](0009-typescript-dictionary-i18n.md)                       | Accepted |
| 0010 | [ネイティブGUI・ターミナル描画基盤への移行検討](0010-evaluate-native-gpui-terminal.md) | Proposed |

## 運用

- 新しい判断は [template.md](template.md) を基に次の番号で記録する。
- Proposedは検討中、Acceptedは採用済み、Rejectedは不採用、Supersededは後続ADRによる置換を表す。
- 判断の変更は新規ADRに記録し、置換元・置換先を相互参照する。既存の経緯は消さない。
- 実装・進捗の詳細は [docs](../README.md) に記載する。
