# ADR-0002: herdr-coreによるセッション管理

- Status: Accepted
- Date: 2026-11-27

## Context

PTY、子プロセス、セッションおよびエージェント状態を管理する必要がある。既存CLIとの共存を優先し、マルチプレクサを独自実装しない。

## Decision

herdr-coreをセッション状態の正とし、v0.1.22に固定する。専用Unixソケットと公開JSON-RPC・Framed Modeを使い、Rustアダプターに依存を集約する。レイアウトと選択状態はuitermが所有する。

## Alternatives Considered

独自実装は既存機能の再実装を要する。既存Herdrの共有インスタンスをそのまま使うとプロセスの所有権が曖昧になる。

## Consequences

更新時にプロトコルと終了処理の互換性を検証する。専用インスタンスを監視し、既定ソケットや他のインスタンスには干渉しない。

GPUIへの移行後もセッション管理の責務と通信境界を維持する。 [ADR-0010](0010-evaluate-native-gpui-terminal.md) を参照。

## References

- [関連する設計・実装記録](../implementation/phase1-backend.md)
