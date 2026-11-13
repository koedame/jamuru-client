# ADR-128: 32bit ARM（armv7 / armhf）の Linux には、アプリ（Tauri）も AppImage と deb でクロスビルドして添える

## Status

Accepted。[ADR-126](./ADR-126-armhf-cli-in-the-release.md) の決定 1（GUI は armhf で出さない）を置き換える。CLI の決定（2〜6）は変わらない。

## Context

[ADR-126](./ADR-126-armhf-cli-in-the-release.md) は、WebKitGTK が NanoPi NEO のメモリに重いことを理由に、armhf には CLI だけを出すと決めた。メンテナーの指示は「GUI（Tauri）も 32bit ARM 向けに出す」で、重さは実機で確かめる話であって、出さない理由ではない。

確かめたこと（Ubuntu 22.04 の amd64 のコンテナ。どこまで測ったかは各項目に書く）:

- armhf の WebKitGTK・GTK・librsvg・OpenSSL の開発パッケージは、`ports.ubuntu.com` の jammy に揃っていて、クロスコンパイラ（`gcc-arm-linux-gnueabihf`）と `PKG_CONFIG_LIBDIR` を armhf に向けるだけで、アプリ（`src-tauri`）がリンクまで通る。依存込みで約 2 分（16 コア）。
- 作れた `jamuru-app` は ELF 32-bit ARM EABI5 で、要求する glibc は 2.34（上限 2.35 の内）。
- `cargo tauri build --bundles deb` は armhf の deb（`Architecture: armhf`、約 14 MB）を作る。
- `cargo tauri build --bundles appimage` は、armhf 用の linuxdeploy と AppRun を自分で取りに行く（Tauri のバンドラが armhf の AppImage に対応している）。ただし linuxdeploy は armhf の実行ファイルなので、動かす側に qemu-user の binfmt が要る（無いと `Exec format error`）。このホストは binfmt を持たないので、AppImage の作成は CI（GitHub のランナー）で確かめた。

## Decision

1. **対象は、アプリ（Tauri）の AppImage と deb。** `release.yml` の `build` ジョブの matrix に `armv7-unknown-linux-gnueabihf` の行（`cross: armhf`）を足す。x64 の Linux と同じ手順（beta の版・updater の設定・サーバー URL の埋め込み・遠隔デバッグの有無の検査・署名・glibc の検査・成果物のアップロード）をそのまま通す。別のジョブにしないのは、署名の鍵と environment（`desktop-signing`）を読むジョブを 1 つに保ち、検査を二重に書かないため。
2. **ネイティブの ARM ランナーは使わない。** `ubuntu-22.04-arm` は 64bit ARM で、32bit のユーザー空間を動かせるかは確かめていない（armv7 の AppImage を作るのに、armhf のコンテナや chroot を足す必要がある）。amd64 の `ubuntu-22.04` でクロスコンパイルし、armhf の実行ファイルは qemu-user-static と binfmt で動かす。ただし `qemu-user-static` が登録する `qemu-arm` は、ELF ヘッダの 8〜15 バイト目が 0 のファイルしか受けない。AppImage の実行ファイル（linuxdeploy と plugin-appimage）はそこに目印（`AI\x02`）を書くので、そのままでは `Exec format error` で動かない（GitHub のランナーで確かめた）。`scripts/register-armhf-binfmt.sh` が、同じ処理系を 7〜15 バイト目を見ない形で登録し直す。`scripts/install-armhf-cross-deps.sh gui` が armhf のライブラリを入れ、クロスの環境変数（`PKG_CONFIG_*`・リンカ・`NO_STRIP`・`APPIMAGE_EXTRACT_AND_RUN`）はその直後の step が `GITHUB_ENV` に書く。`NO_STRIP` は、linuxdeploy が amd64 の `strip` で armhf のライブラリを読もうとして落ちるのを避ける。GTK のプラグインは、GTK のライブラリとツールを「ビルドした機械の」アーキテクチャの場所（`/usr/lib/x86_64-linux-gnu`）に探しに行くので、`LD_GTK_LIBRARY_PATH` と `DEB_HOST_ARCH` で armhf の場所を教える。**`ldd` は armhf を読めない**: linuxdeploy は `ldd` で依存するライブラリを引くが、amd64 の `ldd` は armhf の実行ファイルに「動的リンクされていない」と答え、linuxdeploy は何もコピーしない。AppImage は作れるが、WebKitGTK も GTK も入っていない（実測で 14 パッケージ・17 MB。x64 は 112 パッケージ）。`scripts/armhf-ldd.sh`（armhf のローダーに `--list` させる。qemu-user で動く）を `ldd` として PATH の先頭に置くと、110 パッケージ・77 MB で、x64 と同じ並びになる。AppImage に WebKitGTK が入っていることはリリースのたびに検査する（入っていなければ止める）。
3. **署名し、自動更新に載せる。** AppImage は x64 と同じ鍵で署名し、`latest.json` に `linux-armv7-appimage` として載せる（`tauri-plugin-updater` が armv7 の機器で引くキーが `linux-armv7-appimage`）。載せないと、armhf のアプリが入った機器は手で入れ直すまで更新されない。deb は手で入れる形（x64 と同じ）。
4. **glibc の上限の検査と、成果物がそろっているかの検査に armv7 を足す。** glibc は armhf 用の `objdump`（`OBJDUMP`）で 2.35 まで。成果物の検査は、AppImage と deb の両方が無ければリリースを止める。更新の一覧（`make-update-manifest.sh`）も、armv7 の AppImage とその署名が無ければ書かない。
5. **AppImage に同梱するライブラリの表示（REQ-DIST-008）は armhf の AppImage からも作る。** ビルドの機械は amd64 の同じパッケージも入れているので、`PACKAGE_ARCH=armhf` で armhf のパッケージだけを引く（そうしないと `dpkg-query` が 2 つの版を返して落ちる）。出力は `jamuru-linux-armv7-bundled-libraries.txt` で、x64 の `jamuru-linux-bundled-libraries.txt` と並べて同じリリースに置く。同梱する第三者ライセンスの文（`THIRD_PARTY_LICENSES.md`・`LICENSES.txt`）は両方の名前を書く。
6. **armhf のアプリが作れなくなったことは、毎週 develop で回るリリースの予行（`release.yml` の `schedule`）と、ベータ版のたびに分かる。** 変更のたびに回す検査は置かない（`build.yml` は手で無効にされていて動かない。armhf を毎回のプルリクエストで 10 分以上かけて作るほどの頻度の問題でもない）。ADR-126 が `build.yml` に置いた CLI の検査も、いま動いていない。
7. **opus-codec は付けない**（アプリも CLI も同じ。ADR-126 の決定 3）。

## Consequences

- 毎回のリリースに armhf のアプリが含まれる。armhf の AppImage が作れなくなると、リリース全体が止まる。ビルド時間はリリースの最長のジョブがひとつ増える（qemu 越しの linuxdeploy があるぶん、x64 より遅い）。
- 実機のメモリで画面が動くか、音が途切れないかは確かめていない（NEO の 256〜512MB では重い見込み）。重ければ、CLI の tar.gz（ADR-126）を使う。範囲外。
- armhf の AppImage は、同梱する GTK・WebKitGTK が Ubuntu 22.04 のものなので、要求する glibc は 2.35 まで。Debian 11 など、それより古い OS では起動しない（ADR-126 と同じ）。
- 更新の一覧にキーが 1 つ増える。armhf の AppImage が壊れたリリースは、更新の一覧を書く段階で止まる。

## References

- REQ-DIST-012
- REQ-DIST-008（同梱ライブラリの表示）・REQ-DIST-010（armhf の CLI）
