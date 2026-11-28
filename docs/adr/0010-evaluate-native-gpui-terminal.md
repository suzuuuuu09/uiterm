# ADR-0010: GPUIとネイティブ端末描画の採用

- Status: Accepted
- Date: 2026-11-27（提案）
- Accepted: 2026-11-28

## Context

uitermは軽量性、操作応答性、日本語入力の互換性を重視し、macOSとNixOS / Hyprlandを対象とする。評価時の製品構成はTypeScript、Tauri 2、React、wterm、libghostty-vt（WASM）、herdr-coreである。

Phase 2のmacOS実機検証で、スクロールのカクつき、Neovimの画面更新の遅さ、断続的な描画不具合が確認された。wtermのWebGL描画も試したが、Ghosttyとの差を体感できる程度には解消できず、一部のNerd Fontグリフが表示されなくなった。

統合修正（`5b5a6c4`）の測定では、同一条件のスクロールで25 msを超えたフレームが360フレーム中0件から5件に増えた。試行数が少ないため、この結果だけで性能悪化の原因や現行構成の性能限界は断定できない。macismの実行遅延も別途調査が必要である。

Phase 3以降の機能追加に先立ち、WebViewを介さないGUIと端末描画をPoCで検証した。

## Decision

Rustを主要言語とし、GUIとターミナル描画にGPUI、VT処理にネイティブ版libghostty-vtを採用する。Tauri・React・Zustand・wtermによる製品GUIとWASM版VT処理は移行後の構成から外す。

- herdr-coreをセッション状態の正とし、専用0700ディレクトリ・ソケットとプロセス監視、出力クレジットと再同期、ユーザー入力とVT自動応答の分離を引き継ぐ。境界の入力検証と機密情報の非記録も維持する。
- レイアウトと選択状態はuitermが所有する。端末出力とVT状態はアプリの状態管理から分離する。GUI・CLI・キーバインド・設定・フックには共通Action APIを用い、型付き引数、実行時検証、実行可否、エラー、翻訳キーを引き継ぐ。
- 設定・拡張はTypeScriptと外部Node.jsを使う。JSON-RPC over stdioと未信頼設定の自動実行禁止を維持する。
- Nixによる開発環境と依存固定を維持する。対応対象はmacOS Apple SiliconとNixOS / HyprlandのネイティブWaylandとする。
- 日本語・英語のUI、翻訳キーとロケールに応じた書式を維持する。初回移行ではGPUI端末UIの文言をRustの日英辞書で管理し、OSロケールで選択する。設定による切り替えは内部設定の移植時に扱う。

## 採用根拠

[GPUI PoC検証レポート](../poc/gpui-terminal.md) のリリースビルド測定、自動テスト、ユーザーによるmacOS実機検証を踏まえ、正式採用を決定した。

- 性能測定では、Stage 3時点の256 KiB/sの30秒負荷でGPUI PoCのピークPhysical Footprintは154.51 MiB、比較基準のTauri版は435.44 MiBだった。平均CPUはそれぞれ6.62%と27.30%だった。測定条件とGhosttyとの比較はレポートの§4を参照する。
- 自動検証はIUTF8、IMEイベント、文字幅・Nerd Font、リサイズとAlternate Screen、貼り付け、選択などをカバーした。隔離PTYでのCLI検証と実画面での手動操作は検証範囲が異なる。
- ユーザーによるmacOS実機検証の結果も採用判断に含めた。PoCレポートの§5.1・§8に記載されたチェック項目は検証当時の記録として維持し、今回の確認で個別にどの項目を実施したかは補わない。Tauri版で記録されたNerd Font表示やmacSKKのフォーカス復帰は、その版での結果として扱う。

## Alternatives Considered

- Tauri + wterm DOM / WebGL。DOM版には応答性と描画品質の課題があり、WebGL試用でも改善は十分でなく、一部Nerd Font表示が退行した。
- Tauri + xterm.js WebGL、またはTauri + ネイティブレンダラー。前者ではWebView由来の課題の解消が未検証で、後者ではWebViewとネイティブ描画の統合が必要になる。
- 既存ネイティブレンダラーの組み込みやGhosttyのフォーク。既存の描画資産を使える一方、uitermのGUIとワークスペース・拡張機構との統合が必要になる。PoCで検証したGPUI描画を採用する。

## Consequences

製品版への移行、回帰検証、旧GUIの整理は [ロードマップ](../plan/roadmap.md#phase-2--ターミナル未完了gpui移行) に従う。採用時点ではGPUIは独立したPoCで、Tauri版は比較基準として残る。Phase 2の完了は製品版で判定する。

次の項目は採用決定後の残課題とする。

- NixOS / Hyprland実機でのネイティブWayland、描画、Fcitx5の検証。[MVP完成条件](../plan/mvp.md#完成条件) に含める。
- 可視ウィンドウのアイドル時CPU使用率とGPUIの表示同期通知。計測条件と調査結果はPoCレポートの§4.3を参照する。
- GPUI PoCの依存 `grid 0.18.0` は、GPUI 0.2.2から使うTaffy 0.9.0経由で入る。Taffy 0.9.0は`grid 0.18.0`を要求し、脆弱性の対象は`grid 0.17.0`以上1.0.1未満であるため、修正版への直接更新はできない。GPUI / Taffy側の互換更新を追い、製品移行前に更新可否と回帰を確認する。移行時は外部入力から任意のグリッド寸法を渡さないよう、サイズを境界で検証する。

内蔵ブラウザはPost-MVPとし、`terminal-browser` は候補外とする。

## References

- 置換した判断：[ADR-0001](0001-adopt-typescript-tauri-rust.md)、[ADR-0003](0003-adopt-wterm-ghostty-wasm.md)、[ADR-0004](0004-zustand-action-registry.md)、[ADR-0005](0005-ipc-process-isolation-security.md)、[ADR-0009](0009-typescript-dictionary-i18n.md)
- 継続する判断：[ADR-0002](0002-adopt-herdr-core.md)、[ADR-0006](0006-external-node-typescript-config.md)、[ADR-0007](0007-nix-pnpm-vite-plus.md)、[ADR-0008](0008-pty-iutf8-workaround.md)
- [GPUI PoC検証レポート](../poc/gpui-terminal.md)、[Tauri版Phase 2実装・検証レポート](../implementation/phase2-terminal.md)
- [`grid` の脆弱性 GHSA-38c5-483c-4qqp](https://github.com/advisories/GHSA-38c5-483c-4qqp)、[Taffy 0.9.0の依存定義](https://github.com/DioxusLabs/taffy/blob/v0.9.0/Cargo.toml)、[`grid` 1.0.1](https://github.com/becheran/grid/releases/tag/v1.0.1)
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)、[Ghostty / libghostty](https://github.com/ghostty-org/ghostty)
