# Phase 2 の描画とスクロール調査

macOS 15.5 (24F74) arm64。比較元は main の `5ff26e04ffe441c2c452035c4b40851c653234d2`。修正は `uiterm-rendering-fix` の `fix/terminal-rendering` で行った。Nix の herdr-core は 0.1.22、wterm の core・DOM・Ghostty パッケージはすべて 0.6.0。Ghostty WASM のソースは Ghostty v1.3.1、ネイティブ版の比較対象はインストール済みの Ghostty 1.4.0。wterm の公開最新版は調査時点で 0.6.0、タグ以降の main の3件はドキュメント変更だった。[WASM の依存定義](https://github.com/vercel-labs/wterm/blob/v0.6.0/packages/%40wterm/ghostty/zig/build.zig.zon) · [wterm のタグと main の差分](https://github.com/vercel-labs/wterm/compare/v0.6.0...main) · [herdr-core の変更履歴](https://github.com/herdrdev/herdr-core/blob/master/CHANGELOG.md)

## 背景色と文字幅

隔離 PTY で Codex v0.170.0 に日本語と絵文字を入力し、ローカルのテスト用 Responses API から応答を返した。106列×32行の同じ設定で、採取した VT バイト列を uiterm のアダプターと CSS を読み込んだ macOS の WKWebView に再生した。送信後のユーザーメッセージ行では Codex が `48;2;68;68;68` と行末消去を使い、先頭の `› ` に SGR 2 を付けていた。[Codex v0.170.0 の入力欄実装](https://github.com/openai/codex/blob/rust-v0.170.0/codex-rs/tui/src/bottom_pane/chat_composer.rs)

同じ症状を Codex を使わず再現できる。

```fish
printf '\e[48;2;64;96;144m\e[2KNormal \e[2mDIM \e[22mnormal \e[8mhidden\e[28m\e[0m\n'
```

wterm は通常セルの実色背景を Canvas に、淡色と隠し文字の背景を DOM に描く。修正前は外側のセル span の `opacity:0.5` が背景まで薄くし、`visibility:hidden` が背景も消した。また、一様な背景を行末からグリッド右端の端数幅へ延長する判定が、両属性を除外していた。libghostty が返した背景色は淡色セルと通常セルで同じだった。Ghostty 1.4.0 のネイティブ描画は前景だけに淡色のアルファを適用し、隠し文字の背景も描く。Ghostty の外側のウィンドウ余白は設定で制御される。wterm にも一様な行の端数幅を埋める既存仕様がある。ネイティブ Ghostty のウィンドウは作成できたが、画面取得は macOS に拒まれたため、Ghostty との照合はソースコードと設定に基づく。[Ghostty 1.4.0 の描画処理](https://github.com/ghostty-org/ghostty/blob/v1.4.0/src/renderer/generic.zig#L3192-L3241) · [wterm の背景色修正履歴](https://github.com/vercel-labs/wterm/pull/146)

既存の pnpm パッチに DOM 描画の修正を追加した。淡色と隠し文字の表示属性を文字と装飾に適用し、外側のセルに寸法と背景を保持する。同じ背景色で全列を塗った行は右端まで塗り、異なる背景を含む行は各色のセル境界を保つ。淡色と隠し文字のブロック・罫線には通常の字形描画を使い、文字を覆う特殊な背景や疑似要素で属性を上書きしないようにした。WKWebView の再生で、青いテスト行と Codex の行の色差・右端の欠けが解消した。テスト行の画像では通常、淡色、隠し文字、右端の端数幅から1点ずつ採取した背景画素が、変更後はすべて同じ値になった。描画サイズの再計算や行の再配置は変更していない。

日本語、全角英字、絵文字の後ろの `|` は ASCII 10文字の後ろの `|` と同じ x 座標になった。既定14pxの WKWebView では1セルが 8.65625px、2セルが 17.3125px、一方で `あ` と `Ａ` のブラウザ計測上の字送りは約12.93pxだった。セルの幅と字送りの差で文字間に余白が見える。再現したケースではセル位置の累積ずれを確認できなかったため、フォントと文字幅は変更していない。

## 絵文字と選択

報告の4行を隔離した Tauri リリース版に入力し、専用 herdr-core の画面読み取りでも4行を確認した。macOS WKWebView に同じ出力を描画し、画像でも絵文字を確認した。さらに、幅広の日本語から同じスタイルの絵文字へ置換する操作を修正前後に各80回実行した。既存テキストノードが80回とも再利用され、DOM の文字列に置換違いはなかった。修正後の選択検査では、DOM Range による選択とアダプターからの取得が79回一致した。初回のみブラウザの選択範囲自体が空だった。これはウィンドウを前面化せずに実行した合成操作で観測した。ユーザー報告の選択中の描画崩れやホバーによる復帰は未再現。

画面に誤った字形が出た瞬間の DOM と選択範囲を同時に採取できていない。DOM が正しければフォールバックフォントや WebKit の再描画、DOM まで異なれば wterm の差分更新や core の行データを調べる余地がある。wterm 0.6.0 は更新時に同一スタイルのテキストノードを再利用する。過去の幅広文字の継続セル問題は固定版に修正が入っており、今回測定したセル位置にも該当しなかった。[幅広文字の修正履歴](https://github.com/vercel-labs/wterm/pull/102) · [wterm 0.6.0 の DOM 描画](https://github.com/vercel-labs/wterm/blob/v0.6.0/packages/%40wterm/dom/src/renderer.ts)

## スクロール

同一の WKWebView、939×600 CSS px の端末ホスト、106列×32行、保持履歴2,000行で、`seq 1 10000` 相当の CRLF 区切り出力を1回書き込み、描画完了後に毎フレーム `scrollTop` を23pxずつ120回減らした。ウォームアップ1回を除き3試行。描画時間は `performance.now()` で測った JavaScript 内の計測値で、WebKit の合成・描画時間は含まない。フレーム間隔には描画を待った時間が含まれる。

| 指標 | 変更前 | 変更後 |
| --- | ---: | ---: |
| フレーム間隔 p95（3試行の範囲） | 17 ms | 17–18 ms |
| フレーム間隔最大 | 18 ms | 19 ms |
| 25 ms超過 | 0 / 360 | 0 / 360 |
| wterm の render 回数 / 試行 | 119 | 119 |
| 履歴行の更新回数 / 試行 | 151 | 151 |
| render の累計時間 / 試行 | 8–11 ms | 8–16 ms |

この経路では DOM の更新は仮想化された履歴行の入れ替えに限られ、毎フレームのライブ行再構築は観測しなかった。修正後に147pxずつ動かした場合は980行を更新し、render の累計は81–97ms、フレーム間隔は p95 18ms、最大19ms。どちらもアダプターを直接マウントした試験なので、React コンポーネントの更新は測定対象外。レンダラーはスクロールイベントからフレームを予約し、ビューポートの計測と履歴ウィンドウの更新を行う。wterm upstream #296 は出力を継続しながら画面が流れるときのライブ行再構築の報告で、出力完了後の今回の計測条件とは異なる。公開版を更新する根拠や、スクロール経路を変更する根拠は得られなかった。[wterm の履歴ウィンドウ再利用](https://github.com/vercel-labs/wterm/pull/230) · [wterm #296](https://github.com/vercel-labs/wterm/issues/296)

実機の Tauri 版では専用 PTY に `seq 1 10000` を送り、末尾の `10000` を確認した。ネイティブのホイールイベントを当てる試行ではスクロール位置が動かず、トラックパッドの慣性スクロール、実マウスの選択、OS の貼り付け、実 IME 操作は未確認。症状再現時に WKWebView のスクロール・レイアウト・コンポジットと React のコミットを同時に観測する必要がある。

## 自動検証

- 淡色・隠し文字を含む一様な背景、混在する背景の境界、反転した隠しブロック文字を、実際のパッチ済みレンダラーで検証する回帰テストを3件追加した。
- `pnpm typecheck`、`pnpm lint`、`pnpm test -- --run` は合格。フロントエンド59件。既存の日本語 IME・キーボード・選択周辺のテストを含む。
- Nix 環境での Rust テストは通常実行の25件と、個別に有効化したローカル CLI 統合テスト1件が合格。Tauri の macOS アプリビルドも合格。
