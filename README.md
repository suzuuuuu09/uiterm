# uiterm

**CLI First, Not CLI Only.**

CLIの自由度とGUIの利便性を両立する、日本語環境とキーボード操作を重視したターミナル・ワークスペース管理アプリケーション。

## 開発状況

開発中。Phase 0・1を完了し、Phase 2でGPUIへの移行を進める。MVPは未完成で、macOS Apple SiliconとNixOS / Hyprland（ネイティブWayland）を対象とする。Linux実機検証は未完了。

移行後の製品構成としてRust、GPUI、ネイティブ版libghostty-vt、herdr-coreを正式採用した。TypeScriptによる設定・拡張は外部Node.jsで実行する方針だが、製品機能は未完成である。製品の端末は `src-gpui` にあり、Tauri版は回帰検証と性能比較に使う。[ADR-0010](docs/adr/0010-evaluate-native-gpui-terminal.md) を参照。

## 開発環境

GPUI版はリポジトリルートから起動する。

```sh
nix develop .#gpui --command cargo run --locked --manifest-path src-gpui/Cargo.toml
```

Tauri版の起動と両版の検証は [開発手順](docs/development.md) を参照。

## ドキュメント

- [ドキュメント一覧](docs/README.md)
- [アーキテクチャ](docs/architecture.md)
- [開発状況](docs/status.md)
- [設計判断（ADR）](docs/adr/README.md)
