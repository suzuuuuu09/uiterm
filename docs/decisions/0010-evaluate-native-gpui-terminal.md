# ADR-0010: ネイティブGUI・ターミナル描画基盤への移行検討

- Status: Proposed
- Date: 2026-11-27

## Context

uitermは軽量性、操作応答性、日本語入力の互換性を重視し、macOSとNixOS / Hyprlandを対象とする。現行構成はTypeScript、Tauri 2、React、wterm、libghostty-vt（WASM）、herdr-coreである。

Phase 2のmacOS実機検証で、スクロールのカクつき、Neovimの画面更新の遅さ、断続的な描画不具合が確認された。wtermのWebGL描画も試したが、Ghosttyとの差を体感できる程度には解消できず、一部のNerd Fontグリフが表示されなくなった。

統合修正（`5b5a6c4`）の測定では、同一条件のスクロールで25 msを超えたフレームが360フレーム中0件から5件に増えた。試行数が少ないため、この結果だけで性能悪化の原因や現行構成の性能限界は断定できない。macismの実行遅延も別途調査が必要である。

Phase 3以降の機能追加に先立ち、WebViewを介さないネイティブGUIとターミナル描画の実現性を検証する。

## Decision

Rust + GPUI + ネイティブ版libghostty-vt + herdr-coreを移行先の第一候補とし、独立したPoCで検証する。移行コストではなく、描画性能、入力互換性、リソース消費、将来の拡張性を判断基準とする。

- GPUIをGUI基盤とし、通常のターミナル利用時にWebViewを必要としない構成を検証する。
- libghostty-vtをVT処理の候補として維持する。GPU描画はGPUIによる描画と既存のネイティブレンダラーの再利用を比較し、PoCで方式を決定する。libghostty-vt自体をGhosttyのGPUレンダラーと同一視しない。
- herdr-coreによるPTY・セッション管理、出力と自動応答の分離、共通Action APIの方針を維持する。
- TypeScriptはユーザー設定・拡張に引き続き使用し、外部Node.jsとのJSON-RPC通信方針を維持する。
- macOS Apple SiliconとNixOS / HyprlandのネイティブWayland対応を維持する。
- Phase 3への移行を保留し、既存のPhase 2実装を比較基準として保存する。

内蔵ブラウザはPost-MVPの検討事項とし、本判断の採用条件に含めない。`terminal-browser`は候補から除外する。OS別の内蔵ブラウザ実装方式は本ADRでは決定しない。

## Alternatives Considered

| 選択肢 | 評価 |
| --- | --- |
| Tauri + wterm DOM | 現行構成。実機で操作応答性と描画品質の課題が確認された。比較基準として維持する。 |
| Tauri + wterm WebGL | GPU描画を利用できるが、今回の実機検証では十分な体感改善が得られず、Nerd Font表示も退行した。 |
| Tauri + xterm.js WebGL | 成熟した代替候補。WebViewを介する構成は残り、今回の課題を解消できるかは未検証である。 |
| Tauri + ネイティブレンダラー | ターミナル描画をWebViewから分離できるが、ネイティブ描画とWebViewの統合が必要になる。 |
| Rust + GPUI + ネイティブVT・GPU描画 | WebViewに依存せずGUIと描画を構築できる。macOSとWaylandを対象にできるため第一候補とする。性能とIME互換性は未検証である。 |
| Ghosttyのフォーク | 既存のネイティブ描画と端末互換性を活用できる。一方、uiterm独自のワークスペース管理や拡張機構との統合方法を検討する必要がある。 |

## Consequences

GPUI案が採用された場合、Tauri、React、wtermによるGUI・描画層を置き換え、Rustをアプリケーションの主要言語とする。既存のherdr-coreアダプター、プロトコル、プロセス管理、設定APIなどは責務単位で再利用を評価する。

GPUIとlibghostty-vtのAPI変更、フォントフォールバック、IME、選択範囲、アクセシビリティ、描画同期、Nixでのビルドは新たな検証対象となる。GPUIの採用だけでGhosttyと同等の性能が得られるとは限らない。

PoCでは、リリースビルドを用いて現行uitermおよびGhosttyと同一条件で比較する。

- 入力から描画までの遅延、NeovimのCtrl+D / Ctrl+U、リサイズ、大量出力、スクロール時のフレーム時間。
- アイドル時と負荷時のCPU・メモリ使用量、起動時間、配布サイズ。herdr-coreなどの関連プロセスも含める。
- 日本語IME（macSKK・標準入力）、Nerd Font、全角文字、絵文字、文字選択、コピー・ペースト、fish・Neovim・Codexの互換性。
- herdr-coreとの通信、フロー制御、セッション再同期、非表示タブの負荷。
- Nixによる再現可能なビルドとWayland対応。NixOS / Hyprland実機検証は従来どおりMVP完成条件とする。

採用判断は測定結果と互換性を確認した後に行う。それまでは現行構成に関する既存のADRを置換しない。採用確定時に本ADRのStatus、アーキテクチャ文書、ロードマップおよび関連ADRを更新する。

## References

- [現行アーキテクチャ](https://github.com/suzuuuuu09/uiterm/blob/main/docs/plan/architecture.md)
- [現行ロードマップ](https://github.com/suzuuuuu09/uiterm/blob/main/docs/plan/roadmap.md)
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
- [Ghostty / libghostty](https://github.com/ghostty-org/ghostty)
- [gpui-libghostty](https://github.com/behzade/gpui-libghostty) — ネイティブレンダラーの組み込み例（alpha）。
- [gpui-ghostty](https://github.com/Xuanwo/gpui-ghostty) — libghostty-vtとGPUI描画を組み合わせる実装の参考。
