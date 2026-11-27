# uiterm

**CLI First, Not CLI Only.**

CLIの自由度とGUIの利便性を両立する、日本語環境とキーボード操作を重視したターミナル・ワークスペース管理アプリケーション。

## 開発状況

開発中。Phase 0・1を完了し、Phase 2の機能と描画性能を検証している。MVPは未完成で、macOS Apple SiliconとNixOS / Hyprland（ネイティブWayland）を対象とする。Linux実機検証は未完了。

現在のmainはTauri 2 + React + TypeScript + Rust + herdr-core + wterm / libghostty WASM構成である。ネイティブGUIへの移行は [ADR-0010](decisions/0010-evaluate-native-gpui-terminal.md) で検討中。

## 開発環境

Nixを利用し、リポジトリルートで実行する。

```sh
nix develop --command pnpm install --frozen-lockfile
nix develop --command pnpm tauri dev
```

詳細は [開発手順](docs/development.md) を参照。

## ドキュメント

- [ドキュメント一覧](docs/README.md)
- [アーキテクチャ](docs/architecture.md)
- [開発状況](docs/status.md)
- [設計判断（ADR）](decisions/README.md)
