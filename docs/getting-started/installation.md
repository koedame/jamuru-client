> [!NOTE]
> このドキュメントは開発者向けの解説資料です。
> 正確な仕様・制約・判断は [docs-spec/](../../docs-spec) を参照してください。

# インストール

jamuruをインストールする方法を説明します。

## システム要件

### 対応OS

- Windows 10/11 (64-bit)
- macOS 11.0 (Big Sur) 以降
- Linux (Ubuntu 22.04, Fedora 38 等)

### ハードウェア要件

- オーディオインターフェース（ASIO/CoreAudio/ALSA対応）
- 安定したインターネット接続

> ASIO は Steinberg Media Technologies GmbH の商標です。jamuru は Steinberg と提携・承認の関係にありません。

## インストール方法

### Homebrew（macOS）

Homebrew を使っているなら tap を追加して cask で入れられます。

```bash
brew install --cask koedame/tap/jamuru
```

更新はアプリが自分で行います（後述の「自動更新」）。`brew upgrade --cask jamuru` でも更新できます。削除は `brew uninstall --cask jamuru`（設定ファイルごと消すなら `brew uninstall --zap --cask jamuru`）。

Homebrew はダウンロードしたものに Gatekeeper の隔離属性を付けます。付いたままだと公証なしのアプリは 「"jamuru.app" is damaged and can't be opened.」で開けないため、cask 側でインストール後に属性を外しています。そのため brew で入れた場合は下の「署名なしアプリの警告」の手順は要りません。
cask が指すのは [GitHub Releases](https://github.com/koedame/jamuru-client/releases) に公開済みのタグで、リリースのたびに自動更新されます。

#### ベータ版を試す

正式版より先に動作を確かめたい人向けに、動作を確かめたい変更ごとにベータ版（`v0.2.0-beta.7` のようなタグ）を作り、`jamuru@beta` として配っています。`brew install --cask koedame/tap/jamuru` や `brew upgrade` でベータ版が入ることはなく、正式版を使う人は何もしなくて構いません。

```bash
brew install --cask koedame/tap/jamuru@beta
```

正式版とベータ版は同じ `jamuru.app` を入れるため同時には入れられません。正式版に戻すときは `brew uninstall --cask jamuru@beta` のあとで `brew install --cask koedame/tap/jamuru` を実行してください。ベータ版の更新は `brew upgrade --cask jamuru@beta` です。

### 自動更新

新しい正式版が出ると、jamuru が自分で入れ替えます。操作は要りません。

- 起動すると、画面を開く前に新しい版が出ていないかを GitHub から確かめます。あれば、ダウンロードして署名を確かめ、入れて、新しい版で開き直します。そのあとも 6 時間おきに確かめます。
- 確かめるのに時間がかかったときと、新しい版を入れるときは、小さな窓に進み具合が出ます。急ぐときは「待たずに開く」を押すと、更新をやめてすぐ開きます（入れ始めたあとは、終わるまで待ちます）。招待リンクから開いたときは、確かめずにすぐ開きます。
- **入れた版がうまく起動しなかったときは、前の版に戻ります。** 入れる前に前の版を写して残しておき、新しい版の画面が出て落ち着くまで見届けます。起動しない・すぐ落ちる・固まる、のどれでも、前の版を元どおりに戻して開き直し、その版は自分では入れ直しません（メニューや設定の「アップデートを確認」から頼めば入れます）。
- **セッションの途中では入れません。** セッションを抜けたあとに入れます。
- ベータ版も自動更新されます。次のベータ版が出るとそれに、同じ版の正式版が出たときは正式版に更新されます。beta.17 までのベータ版は自動更新の取得先が正しくなかったので、一度だけ `brew upgrade --cask jamuru@beta` などで入れ替えてください。
- 自動更新に対応するのは、Windows の `.exe`（`-setup.exe`）、macOS、Linux の AppImage です。Windows の `.msi`（全ユーザー向けで、管理者権限なしには入れ替えられません）と Linux の `.deb` は自動更新しません。この 2 つは、新しい版が出ていないかだけを確かめ、出ていれば画面の上に「新しい版が出ています。入れ直してください」と出します。出たら、新しい版を入れ直してください（`.msi` は管理者権限で）。
- 自分でビルドしたアプリは、自動更新しません。

自動更新を待たずに確かめたいときは、メニューの「アップデートを確認…」（macOS は jamuru メニュー、Windows と Linux は「ヘルプ」）か、設定の「アプリについて」の「アップデートを確認」を押します。実行中の版も、設定の「アプリについて」で見られます。新しい版が見つかったら「更新して再起動」で入れられます（自動更新に対応する形で入れたときだけ）。ルームの中にいるあいだは、音が切れるので入れられません。`auto_update = false` にしていても確認できます。

止めるときは、設定ファイル `config.toml` に次の 1 行を足します。設定画面には出しません。

```toml
auto_update = false
```

`config.toml` の場所は [トラブルシューティング](./troubleshooting.md) を参照してください。

#### 旧名 jamjam の版から更新した場合

アプリの名前を jamjam から jamuru に変えたため、設定の置き場も `jamjam` から `jamuru` のフォルダに変わりました。旧名の版の設定・端末の鍵・同意の記録は引き継がれないので、更新後の最初の起動で同意の画面がもう一度出て、端末の ID も新しくなります。旧名のフォルダは使われなくなります。不要なら削除して構いません。

- Linux: `~/.config/jamjam`
- macOS: `~/Library/Application Support/jamjam`
- Windows: `%APPDATA%\jamjam`

### リリースビルドからのインストール

1. [GitHub Releases](https://github.com/koedame/jamuru-client/releases) から最新版をダウンロード
2. 各プラットフォーム用のインストーラを実行:
   - Windows: `.exe`（自動更新する）または `.msi`（全ユーザー向け・自動更新しない）
   - macOS: `.dmg`
   - Linux: `.AppImage` または `.deb`

> [!CAUTION]
> **署名なしアプリの警告について**
> 現在配布しているビルドはコード署名されていないため、OSのセキュリティ機能により警告が表示されます。
> 以下の手順で起動できます。

#### Windows での起動方法

Windows Defender SmartScreen の警告が表示された場合:

1. 「詳細情報」をクリック
2. 「実行」ボタンをクリック

#### macOS での起動方法

初回起動時に「"jamuru.app"は開かれませんでした。Appleは"jamuru.app"にMacに損害を与えたり
プライバシーを侵害する可能性のあるマルウェアが含まれていないことを確認できませんでした。」と
表示された場合（このダイアログを閉じるボタンしかなく、ここからは開けません）:

1. **システム設定** → **プライバシーとセキュリティ**
2. 「jamuruは開発元を確認できないため、使用がブロックされました」の横にある「このまま開く」をクリック

macOS 14 (Sonoma) 以前では、Finder でアプリケーションを右クリック（または Control + クリック）して
「開く」を選ぶ方法も使えます。**macOS 15 (Sequoia) 以降ではこの右クリックからの手順は廃止されており、
上記のシステム設定からの許可が唯一の方法です。**

#### Linux での起動方法

AppImage の場合、実行権限を付与してから起動:

```bash
chmod +x jamuru_*.AppImage
./jamuru_*.AppImage
```

## 次のステップ

インストールが完了したら、[クイックスタート](quick-start.md)に進んでください。

> [!NOTE]
> **プライバシーについて**
> jamuruはP2P通信を使用するため、セッション参加者間でIPアドレスが共有されます。
> 詳細は[プライバシーとセキュリティ](privacy.md)をご確認ください。
