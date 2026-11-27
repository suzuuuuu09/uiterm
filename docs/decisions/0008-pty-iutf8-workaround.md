# ADR-0008: PTYのIUTF8暫定対応

- Status: Accepted
- Date: 2026-11-27

## Context

herdr-core v0.1.22の新規PTYではIUTF8が有効にならず、カノニカルモードのBackspaceでUTF-8がバイト単位に消去される。macOSで文字化けを再現した。

## Decision

新規PTYのコマンドをuiterm自身の --uiterm-pty-init モードでラップする。Tokio・Tauriの起動前にtermiosのIUTF8だけを有効化し、execで対象プログラムへ置換する。公開argvから内部ラッパーを除去する。

## Alternatives Considered

herdr-coreのソースビルドによるパッチはZigとGhosttyサブモジュールのビルドを要する。ユーザー側のstty設定は全セッションでの動作を保証しない。

## Consequences

macOS実機と固定版coreの統合テストで修正を確認した。上流が新規PTYでIUTF8を設定する版に更新した際は回帰テストを維持して暫定ラッパーを撤去する。

## References

- [関連する設計・実装記録](../docs/implementation/phase2-terminal.md)
