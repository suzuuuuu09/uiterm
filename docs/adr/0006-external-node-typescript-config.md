# ADR-0006: 外部Node.jsによるTypeScript設定

- Status: Accepted
- Date: 2026-11-27

## Context

型安全な設定、関数、独自アクション、フックを記述できるようにし、不要なランタイムを同梱しない。

## Decision

外部Node.js 24 LTSを利用し、設定ランタイムとアプリ間をJSON-RPC over stdioで分離する。Node.jsは通常起動の必須依存にしない。信頼できない設定を自動実行しない。

## Alternatives Considered

独自処理系やランタイムの同梱は配布容量と保守負担を増やす。静的設定だけでは任意の関数による拡張を満たさない。

## Consequences

Phase 0で方式を検証済み。現在は明示的に有効化した開発用PoCに限り、製品向けAPI、信頼付与、復旧はPhase 4で扱う。

## References

- [関連する設計・実装記録](../implementation/phase1-backend.md)
