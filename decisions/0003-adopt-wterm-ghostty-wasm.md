# ADR-0003: wterm・Ghostty WASMによる端末描画

- Status: Accepted
- Date: 2026-11-27

## Context

既存ライブラリでVT処理、日本語・Unicode文字幅、入力互換性を実現し、セッション管理と描画を分離する。

## Decision

現行GUIでは @wterm/dom と @wterm/ghostty を採用し、libghostty-vtのWASMバックエンドでVTを処理する。出力はTerminalAdapterへ直接渡し、React・Zustandに流さない。既定スクロールバックは2,000行とする。

## Alternatives Considered

gespenst・xterm.jsを代替候補とする。ネイティブ描画は [ADR-0010](0010-evaluate-native-gpui-terminal.md) で再評価する。

## Consequences

herdr-core側と描画側にVT処理が存在するため、CPU・メモリ負荷を測定する。WebView固有の描画・IME不具合や上流パッチを管理する。

## References

- [関連する設計・実装記録](../docs/implementation/phase2-terminal.md)
