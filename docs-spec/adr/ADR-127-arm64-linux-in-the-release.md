# ADR-127: 64bit ARM（aarch64 / arm64）の Linux には、アプリ（AppImage・deb）と CLI を arm64 のランナーでビルドしてリリースに添える

## Status

Accepted。

## Context

NanoPi R3S 2GB（RK3566、Cortex-A55 ×4、64bit ARM、メモリ 2GB）で jamuru を動かしたい。リリースの Linux 向け成果物は amd64 の AppImage と deb、32bit ARM の CLI（[ADR-126](./ADR-126-armhf-cli-in-the-release.md)）だけで、arm64 では動かせない。

R3S のメモリは WebKitGTK を動かすのに足りる見込みなので、CLI だけでなく画面を持つアプリも出す（メンテナーの指示、2026-11-13）。GUI を外す判断はしない。

## Decision

1. **ビルドは GitHub の arm64 ランナー（`ubuntu-22.04-arm`）でネイティブに行う。** クロスコンパイルも qemu も使わない。アプリ（Tauri）は WebKitGTK など多くのシステムのライブラリを AppImage に取り込むので、そのライブラリがある arm64 の環境で作るのが確実で、AppImage を作る道具（Tauri のバンドラ）の arm64 版もネイティブで動く。ランナーのイメージは amd64 と同じ Ubuntu 22.04 なので、要求する glibc は同じ 2.35 までに揃う（`check-glibc-floor.sh`）。リポジトリは公開なので arm64 のランナーに費用はかからない。
2. **アプリは AppImage と deb を作り、毎回のリリースに含める。** `release.yml` の `build` のマトリクスに `aarch64-unknown-linux-gnu` を足す。ファイル名は Tauri が決める `jamuru_<版>_aarch64.AppImage` と `jamuru_<版>_arm64.deb` で、amd64 のものと衝突しない。AppImage に取り込んだシステムのライブラリの表示（REQ-DIST-008）は、衝突しないよう `jamuru-linux-arm64-bundled-libraries.txt` として添える。同梱する `LICENSES.txt` はこの名前も案内する。Tauri の CLI は arm64 Linux 向けのビルド済みが配られていないので、`cargo install --locked` で作り、週次のリハーサルが保存したキャッシュを使う。
3. **CLI は `jamuru-cli-linux-aarch64.tar.gz` と `.sha256` を添える**（ADR-126 の armv7 と同じ形。中身は `jamuru`・`LICENSE`・`THIRD_PARTY_LICENSES.md`）。`build-cli` ジョブをマトリクスにし、aarch64 はネイティブでビルドして、そのまま `--version` と `--help` を動かしてから公開する。署名と自動更新は無い。秘密を読まず、書き込み権限も持たない（ADR-126 の 6 と同じ）。
4. **CLI の機能は既定のまま（`opus-codec` を付けない）。** どのプリセットも Opus を使わず（ADR-021）、アプリも付けずに配っているので、CLI だけ増やさない。aarch64 は NEON が標準なので、armv7 のように libopus のビルドで落ちることはない。付ける必要が生まれたら別の決定として扱う。
5. **自動更新の情報（`latest.json`）に arm64 の AppImage を載せる（キーは `linux-aarch64-appimage`）。** 更新の署名は amd64 と同じ鍵で、Tauri のアップデーターは実行中の CPU の種類のキーを引く。載せないと、arm64 の AppImage が更新を確かめるたびに該当するキーが無くて失敗するので、載せる。載せる以上、更新が起動しなかったときに前の版へ戻る仕組み（ADR-089）も arm64 で確かめる: 更新ドリル（`update-drill.yml`）に arm64 の Linux（`ubuntu-22.04-arm`）を足し、リリースは arm64 でも通ってから公開する。更新の自動入れ替えに対応しない deb が新しい版を知らせる経路（`notice_target`）も、CPU の種類のキーを引くよう直す。arm64 の AppImage が欠けると `make-update-manifest.sh` が `latest.json` を書かずにリリースを止める。
6. **変更のたびに回すのは CLI（`build.yml`）だけ。** アプリの arm64 のビルドは、`release.yml` の週次のリハーサル（develop）とリリースで回る。アプリのビルドは長く、arm64 のランナーを毎回占有するほどの頻度は要らないと見た。arm64 でアプリが作れなくなったら、次のリハーサルで落ちて分かる。

## Consequences

- 毎回のリリースに arm64 のアプリと CLI が含まれる。どれかのビルドが落ちればリリース全体が止まる。
- 要求する glibc は 2.35 まで。Ubuntu 22.04・Debian 12 より古い OS では起動しない。
- 初回の arm64 のビルドは Tauri の CLI を作るぶん長い。週次のリハーサルがキャッシュを保存したあとは短くなる。
- 更新ドリルのジョブが 2 つ（現在の版から・前のリリースから）増える。
- 実機（R3S）での音と、小さいバッファの処理が間に合うかは検証範囲に入れない。R3S は HDMI も音声端子も持たないので、実機で試すときは USB のオーディオインターフェースが前提になる。
- 他の 64bit ARM の Linux（Raspberry Pi の 64bit OS など）でも動く見込みだが、検証したのは CI の arm64 ランナーまで。

## References

- REQ-DIST-011
- ADR-089（更新の見届けと巻き戻し）、ADR-126（armv7 の CLI）
