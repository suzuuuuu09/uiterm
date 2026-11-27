# ADR-0004: Zustand Vanilla・Action Registry

- Status: Accepted
- Date: 2026-11-27

## Context

GUI、CLI、キーバインド、設定、フックから同じ操作を呼び出せるようにする。表示状態と端末出力を分離する。

## Decision

Zustand VanillaでWindow → Workspace → Tab → Pane、選択、設定、セッションのミラーを管理する。Action Registryで引数の型、実行時検証、実行可否、エラー、翻訳キーを共通化する。端末出力やVT状態はストアに保持しない。

## Alternatives Considered

Reactコンポーネントに操作と状態を閉じ込める案はCLIや設定からの再利用を難しくする。

## Consequences

外部イベントの順序とスナップショット再同期を扱う。GUI基盤を変更しても共通操作APIの責務は維持する。

## References

- [関連する設計・実装記録](../docs/implementation/phase1-frontend.md)
