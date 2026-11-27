# AGENTS.md

## 作業前に読む文書

- [開発状況](docs/status.md)
- [アーキテクチャ](docs/architecture.md)
- [開発手順](docs/development.md)
- [通信・操作契約](docs/interfaces.md)
- [ADR一覧](decisions/README.md)

## 実装上の制約

- CLI First, Not CLI Only。独自AIハーネスを実装せず、既存CLIを利用する。
- 現行実装ではRustとTypeScriptの責務を分離し、端末出力をReact・Zustandに流さない。
- herdr-coreをセッション状態の正とし、ユーザー入力と自動応答を混同しない。
- 他のインスタンスを停止したり、既定のHerdrソケットを利用したりしない。
- 未信頼設定を自動実行しない。端末入力、IME文字列、認証情報をログに保存しない。
- 日本語IME、Nerd Font、Unicode幅、macOS・ネイティブWayland対応を退行させない。
- pnpm・Cargo・Nixを使用し、固定版とロックファイルの整合を維持する。
- 実装変更に対応するテストを追加し、検証済み・未検証を区別する。

## 開発の進め方

Phase 3はPhase 2の検証と [ADR-0010](decisions/0010-evaluate-native-gpui-terminal.md) の評価が済むまで保留する。mainのコードと既存の実装レポートを参照し、ローカルの検証結果をマージ済みと扱わない。

重要な設計変更はADRに記録する。検証手順は [docs/development.md](docs/development.md) を参照。
