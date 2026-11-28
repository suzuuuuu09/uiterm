# ADR-0007: Nix・pnpm・Vite+の採用

- Status: Accepted
- Date: 2026-11-27

## Context

macOSとNixOSの開発環境を再現可能にし、依存固定と検証手順を統一する。

## Decision

Nix Flakesで環境を管理し、TypeScriptはpnpmとVite+、RustはCargoを利用する。ロックファイルを管理し、herdr-coreを版とハッシュで固定する。NixOSではネイティブWaylandを対象とする。

## Alternatives Considered

OSごとの手動導入は環境差を増やす。Chromiumや不要なランタイムを同梱しない。

## Consequences

Flake評価と実機動作を区別する。Linux実機はMVP完成条件。

Nix・Cargo・pnpmと依存固定を維持する。Vite+は既存Tauri版で引き続き使用し、設定・拡張の開発で必要なツールは移行時に見直す。 [ADR-0010](0010-evaluate-native-gpui-terminal.md) を参照。

## References

- [関連する設計・実装記録](../plan/architecture.md)
