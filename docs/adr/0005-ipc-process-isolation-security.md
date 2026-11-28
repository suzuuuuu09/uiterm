# ADR-0005: IPC・プロセス隔離・セキュリティ

- Status: Superseded
- Date: 2026-11-27

GPUI製品版への移行後、Tauri固有のIPCと権限境界は [ADR-0010](0010-evaluate-native-gpui-terminal.md) により置換する。移行完了までは現行Tauri版に本ADRの制御を適用し、herdr-coreとの通信、プロセス隔離、フロー制御、入力検証と機密情報の非記録は移行後も引き継ぐ。

## Context

端末入出力は高頻度であり、GUIのクラッシュや切断時にも安全にプロセスを管理する必要がある。WebViewに任意の特権APIを公開しない。

## Decision

制御にはTauri CommandsとJSON、端末入出力にはRaw Binary IPCを用いる。ユーザー入力とVT自動応答を別種別で転送し、クレジット制で流量を制御する。Rust所有DTOはts-rsで生成し外部入力を実行時検証する。専用0700ディレクトリ・ソケット・監視プロセス、Capabilities、CSP、遷移制限を適用する。端末入力・IME文字列等をログに残さない。

## Alternatives Considered

JSON・Base64による全出力転送は余分な処理を要する。共有ソケットやプロセス名による一括終了は所有権を明確にできない。

## Consequences

再同期、境界検証、監視プロセスの終了処理が必要。同一UIDのプログラムを隔離するサンドボックスではない。外部Webページはアプリの特権APIから分離する。

## References

- [関連する設計・実装記録](../implementation/phase1-backend.md)
