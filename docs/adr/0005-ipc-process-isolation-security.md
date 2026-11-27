# ADR-0005: IPC・プロセス隔離・セキュリティ

- Status: Accepted
- Date: 2026-11-27

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
