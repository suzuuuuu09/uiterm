# ADR-0003: wterm・Ghostty WASMによる端末描画

- Status: Superseded
- Date: 2026-11-27

端末描画とVT処理の選択は [ADR-0010](0010-evaluate-native-gpui-terminal.md) により置換された。セッション管理と描画の分離は引き継ぐ。

## Context

既存ライブラリでVT処理、日本語・Unicode文字幅、入力互換性を実現し、セッション管理と描画を分離する。

## Decision

現行GUIでは @wterm/dom と @wterm/ghostty を採用し、libghostty-vtのWASMバックエンドでVTを処理する。出力はTerminalAdapterへ直接渡し、React・Zustandに流さない。既定スクロールバックは2,000行とする。

## Alternatives Considered

gespenst・xterm.jsを代替候補とする。ネイティブ描画も比較対象とした。

## Consequences

herdr-core側と描画側にVT処理が存在するため、CPU・メモリ負荷を測定する。WebView固有の描画・IME不具合や上流パッチを管理する。

## References

- [関連する設計・実装記録](../implementation/phase2-terminal.md)
