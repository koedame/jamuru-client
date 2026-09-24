---
sidebar_position: 1
title: インストール
description: jamjamのインストール方法
---

:::note
このドキュメントは開発者向けの解説資料です。
正確な仕様・制約・判断は [docs-spec/](https://github.com/koedame/jamjam-client/tree/main/docs-spec) を参照してください。
:::

# インストール

jamjamをインストールする方法を説明します。

## システム要件

### 対応OS

- Windows 10/11 (64-bit)
- macOS 11.0 (Big Sur) 以降
- Linux (Ubuntu 22.04, Fedora 38 等)

### ハードウェア要件

- オーディオインターフェース（ASIO/CoreAudio/ALSA対応）
- 安定したインターネット接続

## インストール方法

### Homebrew（macOS）

Homebrew を使っているなら tap を追加して cask で入れられます。

```bash
brew install --cask koedame/tap/jamjam
```

更新はアプリが自分で行います（後述の「自動更新」）。`brew upgrade --cask jamjam` でも更新できます。削除は `brew uninstall --cask jamjam`（設定ファイルごと消すなら `brew uninstall --zap --cask jamjam`）。

Homebrew はダウンロードしたものに Gatekeeper の隔離属性を付けます。付いたままだと公証なしのアプリは 「"jamjam.app" is damaged and can't be opened.」で開けないため、cask 側でインストール後に属性を外しています。そのため brew で入れた場合は下の「署名なしアプリの警告」の手順は要りません。
cask が指すのは [GitHub Releases](https://github.com/koedame/jamjam-client/releases) に公開済みのタグで、リリースのたびに自動更新されます。

#### ベータ版を試す

正式版より先に動作を確かめたい人向けに、main ブランチにマージされるたびにベータ版（`v0.2.0-beta.7` のようなタグ）を作り、`jamjam@beta` として配っています。`brew install --cask koedame/tap/jamjam` や `brew upgrade` でベータ版が入ることはなく、正式版を使う人は何もしなくて構いません。

```bash
brew install --cask koedame/tap/jamjam@beta
```

正式版とベータ版は同じ `jamjam.app` を入れるため同時には入れられません。正式版に戻すときは `brew uninstall --cask jamjam@beta` のあとで `brew install --cask koedame/tap/jamjam` を実行してください。ベータ版の更新は `brew upgrade --cask jamjam@beta` です。

### 自動更新

新しい正式版が出ると、jamjam が自分で入れ替えます。操作は要りません。

- 起動の少しあとと、その後 6 時間おきに、新しい版が出ていないかを GitHub から確かめます。あれば、ダウンロードして署名を確かめ、入れて、再起動します。
- **セッションの途中では入れません。** セッションを抜けたあとに入れます。
- ベータ版どうしは自動更新されません（上の「ベータ版を試す」）。ベータ版を使っていて、それより新しい正式版が出たときは、正式版に更新されます。
- 対応するのは、Windows（`.msi`・`.exe`）、macOS、Linux の AppImage です。Linux の `.deb` で入れた場合は、パッケージ管理（`apt`）で更新してください。
- 自分でビルドしたアプリは、自動更新しません。

止めるときは、設定ファイル `config.toml` に次の 1 行を足します。設定画面には出しません。

```toml
auto_update = false
```

`config.toml` の場所は [トラブルシューティング](./troubleshooting.md) を参照してください。

### リリースビルドからのインストール

1. [GitHub Releases](https://github.com/koedame/jamjam-client/releases) から最新版をダウンロード
2. 各プラットフォーム用のインストーラを実行:
   - Windows: `.msi` または `.exe`
   - macOS: `.dmg`
   - Linux: `.AppImage` または `.deb`

:::caution 署名なしアプリの警告について
現在配布しているビルドはコード署名されていないため、OSのセキュリティ機能により警告が表示されます。
以下の手順で起動できます。
:::

#### Windows での起動方法

Windows Defender SmartScreen の警告が表示された場合:

1. 「詳細情報」をクリック
2. 「実行」ボタンをクリック

#### macOS での起動方法

初回起動時に「"jamjam.app"は開かれませんでした。Appleは"jamjam.app"にMacに損害を与えたり
プライバシーを侵害する可能性のあるマルウェアが含まれていないことを確認できませんでした。」と
表示された場合（このダイアログを閉じるボタンしかなく、ここからは開けません）:

1. **システム設定** → **プライバシーとセキュリティ**
2. 「jamjamは開発元を確認できないため、使用がブロックされました」の横にある「このまま開く」をクリック

macOS 14 (Sonoma) 以前では、Finder でアプリケーションを右クリック（または Control + クリック）して
「開く」を選ぶ方法も使えます。**macOS 15 (Sequoia) 以降ではこの右クリックからの手順は廃止されており、
上記のシステム設定からの許可が唯一の方法です。**

#### Linux での起動方法

AppImage の場合、実行権限を付与してから起動:

```bash
chmod +x jamjam_*.AppImage
./jamjam_*.AppImage
```

### ソースからのビルド

開発版を使用する場合は、ソースからビルドします。

```bash
# リポジトリをクローン
git clone https://github.com/koedame/jamjam-client.git
cd jamjam-client

# Rustコアのビルド
cargo build --release

# Tauri GUIビルド（プロジェクトルートから実行。使うサーバーを渡す）
JAMJAM_SERVER_URL=https://<サーバー> cargo tauri build
```

#### ビルド依存関係

**Ubuntu/Debian:**
```bash
sudo apt-get update
sudo apt-get install -y libasound2-dev libssl-dev libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

**macOS:**
- Xcode Command Line Tools

**Windows:**
- Visual Studio Build Tools (MSVC)

## 次のステップ

インストールが完了したら、[クイックスタート](/docs/getting-started/quick-start)に進んでください。

:::info プライバシーについて
jamjamはP2P通信を使用するため、セッション参加者間でIPアドレスが共有されます。
詳細は[プライバシーとセキュリティ](/docs/getting-started/privacy)をご確認ください。
:::
