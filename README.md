# uiterm

**CLI First, Not CLI Only.**

CLIの自由度とGUIの利便性を両立する、日本語環境とキーボード操作を重視したターミナル・ワークスペース管理アプリケーション。

## 開発状況

開発中。Phase 0・1を完了し、Phase 2でGPUIへの移行を進める。MVPは未完成で、macOS Apple SiliconとNixOS / Hyprland（ネイティブWayland）を対象とする。Linux実機検証は未完了。

製品構成としてRust、GPUI、ネイティブ版libghostty-vt、herdr-coreを正式採用した。TypeScriptによる設定・拡張は外部Node.jsで実行する。現在の製品実装はTauri版で、GPUI版は独立したPoCとして収録している。[ADR-0010](docs/adr/0010-evaluate-native-gpui-terminal.md) を参照。

## 開発環境

既存のTauri版はNixを利用し、リポジトリルートで起動する。

```sh
nix develop --command pnpm install --frozen-lockfile
nix develop --command pnpm tauri dev
```

GPUI PoCの起動方法を含む詳細は [開発手順](docs/development.md) を参照。

## ドキュメント

- [ドキュメント一覧](docs/README.md)
- [アーキテクチャ](docs/architecture.md)
- [開発状況](docs/status.md)
- [設計判断（ADR）](docs/adr/README.md)
