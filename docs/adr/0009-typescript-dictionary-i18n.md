# ADR-0009: TypeScript辞書による国際化

- Status: Accepted
- Date: 2026-11-27

## Context

日本語を基本要件とし、日本語・英語のUIを提供する。翻訳キーの欠落と書式の不統一を防ぐ。

## Decision

日本語のTypeScript辞書を翻訳キーの正とし、英語辞書にも同一キーを型で強制する。数値・日時・相対時間等はIntl APIを用い、エラー・アクション・UI文言を翻訳キーで管理する。

## Alternatives Considered

i18next等は現時点の要件に対する追加依存となる。

## Consequences

キー一致を型とテストで検証する。複雑な翻訳要件が生じた場合は再評価する。ネイティブ移行時も日本語・英語を維持し実装方式は再検討する。

## References

- [関連する設計・実装記録](../implementation/phase1-frontend.md)
