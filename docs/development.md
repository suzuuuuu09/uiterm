# 開発・検証手順

GPUI製品版と比較基準のTauri版の手順。構成は [ADR-0010](adr/0010-evaluate-native-gpui-terminal.md) を参照。コマンドはリポジトリルートで実行する。

## Tauri版の起動

```sh
nix develop --command pnpm install --frozen-lockfile
nix develop --command pnpm tauri dev
```

## GPUI製品版の起動

開発時は `cargo run`、手動検証と性能比較はリリースビルドを使う。

```sh
nix develop .#gpui --command cargo run --locked --manifest-path src-gpui/Cargo.toml
nix develop .#gpui --command cargo build --locked --release --manifest-path src-gpui/Cargo.toml
nix develop .#gpui --command ./src-gpui/target/release/uiterm --font-preset menlo --font-size 14
```

開発シェル外では `uiterm` と同じディレクトリに固定版 `herdr-core` v0.1.22 を置くか、`HERDR_CORE_BIN` に実行ファイルの絶対パスを指定する。Linuxでの実行にはGPUIのフォント・Vulkan・Waylandの実行時依存も必要。

実機検証と診断の手順は [GPUI PoC検証レポート §8](poc/gpui-terminal.md#8-実機検証手順と起動コマンド) を参照。

## 自動検証

```sh
nix develop --command cargo fmt --check --manifest-path src-tauri/Cargo.toml
nix develop .#gpui --command cargo fmt --check --manifest-path src-gpui/Cargo.toml
nix develop --command pnpm check
nix develop --command pnpm typecheck
nix develop --command pnpm test
nix develop --command pnpm build
nix develop --command cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
nix develop .#gpui --command cargo clippy --locked --manifest-path src-gpui/Cargo.toml --all-targets -- -D warnings
nix develop --command cargo test --locked --manifest-path src-tauri/Cargo.toml
nix develop .#gpui --command cargo test --locked --manifest-path src-gpui/Cargo.toml
nix develop --command cargo build --locked --manifest-path src-tauri/Cargo.toml
nix develop .#gpui --command cargo build --locked --manifest-path src-gpui/Cargo.toml
nix flake check --no-build --all-systems --no-write-lock-file
```

## GitHub Actions CI

`.github/workflows/ci.yml` はPull Requestとmainへのpushで実行する。Rustfmt、Clippy、Rustテスト、TauriバックエンドとGPUI製品版のデバッグ・リリースビルドとテストはmacOS ARM64（`macos-15`）とLinux x86_64（`ubuntu-24.04`）で行う。TypeScriptの型検査とVite+のcheck・test・build、Flakeの全対象システム評価はLinuxで一度だけ行う。

CIは既存Flakeの開発シェルを使い、CargoとpnpmのダウンロードキャッシュにはGitHub Actions cacheを使う。Nixのビルド済み依存は既定の`cache.nixos.org`を使う。全ActionはコミットSHAで固定し、Cargo・pnpm・Flakeのロックファイルを変更しない。

Flake評価やmacOS/Linuxランナーでのビルドは、NixOS/Hyprland実機、GUI、IME、Nerd Font、クリップボード、連続利用や性能の検証を代替しない。実機検証は「検証上の注意」に記載した手順で行う。

## Tauri版のmacOS配布ビルド

```sh
nix develop --command pnpm tauri build --bundles app
```

herdr-coreは固定版0.1.22を利用する。現在は外部依存で、配布サイズ評価ではcoreも含める。開発シェル外での起動経路と配布方法は今後の課題である。

## 検証上の注意

- 手動検証ではfish、Neovim、Codex、IME、Nerd Font、コピー・ペースト、リサイズを確認する。
- 性能はリリースビルド、同じグリッド・フォント・履歴・負荷条件で測定する。関連プロセスを含め、体感と測定値を区別する。
- テスト用HOME・XDGを隔離する。他のHerdrや実行中のプロセスを停止しない。
- Flakeの静的評価はLinux実機検証の代わりにならない。

詳細：[Phase 1 Rust](implementation/phase1-backend.md)、[Phase 2検証レポート](implementation/phase2-terminal.md)。
