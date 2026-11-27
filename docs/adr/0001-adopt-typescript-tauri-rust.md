# ADR-0001: TypeScript・Tauri・Rustの採用

- Status: Accepted
- Date: 2026-11-27

## Context

GUIとCLIの利点を両立しながらmacOSとLinuxで軽量なアプリを構築する。日本語入力とOS連携も必要になる。

## Decision

TypeScriptを主要言語、ReactをUI、Tauri 2をデスクトップ基盤とし、RustはOS連携・プロセス管理・IPCを担当する。React Compiler、Tailwind CSS v4、Base UIを利用する。

## Alternatives Considered

ElectronはChromiumを同梱するため軽量性の目標と合わない。RustのネイティブGUIに対して、当初はTypeScript中心の開発を優先した。

## Consequences

WebViewの性能とOSごとの入力互換性に依存する。Phase 2で見つかった問題により [ADR-0010](0010-evaluate-native-gpui-terminal.md) でネイティブ構成を再評価する。採用確定までは本ADRを維持する。

## References

- [関連する設計・実装記録](../plan/architecture.md)
