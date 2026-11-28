# ADR-0001: TypeScript・Tauri・Rustの採用

- Status: Superseded
- Date: 2026-11-27

製品GUIと主要言語の選択は [ADR-0010](0010-evaluate-native-gpui-terminal.md) により置換された。Tauri版の実装は移行中の比較基準として残る。

## Context

GUIとCLIの利点を両立しながらmacOSとLinuxで軽量なアプリを構築する。日本語入力とOS連携も必要になる。

## Decision

TypeScriptを主要言語、ReactをUI、Tauri 2をデスクトップ基盤とし、RustはOS連携・プロセス管理・IPCを担当する。React Compiler、Tailwind CSS v4、Base UIを利用する。

## Alternatives Considered

ElectronはChromiumを同梱するため軽量性の目標と合わない。RustのネイティブGUIに対して、当初はTypeScript中心の開発を優先した。

## Consequences

WebViewの性能とOSごとの入力互換性に依存する。Phase 2で見つかった問題を受けてネイティブ構成を評価した。

## References

- [関連する設計・実装記録](../plan/architecture.md)
