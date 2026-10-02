> [!NOTE]
> このドキュメントは開発者向けの解説資料です。
> 正確な仕様・制約・判断は [docs-spec/](../../docs-spec) を参照してください。

# ビルド

jamjamをソースからビルドする方法を説明します。権利者（開発者）向けの手順です。[ライセンス](../../LICENSE)は、権利者以外がソースからビルド・実行することを許可していません。利用するときは、GitHub Releases の公式バイナリを使ってください。

## 前提条件

### Rust

```bash
# Rust のインストール（rustup推奨）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# stable チャンネルを使用
rustup default stable
```

### Tauri CLI

```bash
cargo install tauri-cli --version "^2"
```

### OS別依存関係

**Ubuntu/Debian:**
```bash
sudo apt-get update
sudo apt-get install -y \
  libasound2-dev \
  libssl-dev \
  libwebkit2gtk-4.1-dev \
  libappindicator3-dev \
  librsvg2-dev \
  patchelf
```

**macOS:**
```bash
xcode-select --install
```

**Windows:**
- Visual Studio Build Tools (MSVC) をインストール

## ビルドコマンド

### Rustコアのみ

```bash
# デバッグビルド
cargo build

# リリースビルド
cargo build --release
```

### Tauri GUI (デスクトップアプリ)

> [!CAUTION]
> Tauriコマンドは必ず**プロジェクトルート**から実行してください。`src-tauri` ディレクトリからの実行はサポートされていません（[ADR-009](../../docs-spec/adr/ADR-009-tauri-build-commands.md) 参照）。

```bash
# 開発サーバー起動（プロジェクトルートから実行）
cargo tauri dev

# リリースビルド（プロジェクトルートから実行。使うサーバーを渡す）
JAMJAM_SERVER_URL=https://<サーバー> cargo tauri build
```

> [!NOTE]
> 開発ビルド（`cargo tauri dev`）は `http://localhost:17890` のサーバーを使います。リリースビルドは `JAMJAM_SERVER_URL` で渡したサーバーを使い、渡さなければビルドが失敗します。どちらも、シグナリングサーバーの接続先は繋ぐたびにそのサーバーへ問い合わせます。

### 成果物の場所

| OS | 形式 | パス |
|----|------|------|
| Windows | `.msi`, `.exe` | `src-tauri/target/release/bundle/msi/` |
| macOS | `.dmg`, `.app` | `src-tauri/target/release/bundle/dmg/` |
| Linux | `.AppImage`, `.deb` | `src-tauri/target/release/bundle/` |

## トラブルシューティング

### ビルドエラー: libasound not found

```bash
# Ubuntu/Debian
sudo apt-get install libasound2-dev
```

### ビルドエラー: webkit2gtk not found

```bash
# Ubuntu/Debian
sudo apt-get install libwebkit2gtk-4.1-dev
```

## 関連情報

- [テスト](testing.md) - テストの実行方法
- [CI/CD](ci.md) - 継続的インテグレーション
