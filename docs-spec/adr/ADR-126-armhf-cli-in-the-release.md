# ADR-126: 32bit ARM（armv7 / armhf）の Linux には、CLI だけをクロスビルドして tar.gz でリリースに添える

## Status

Accepted。決定 1（GUI は出さない）は [ADR-128](./ADR-128-armhf-app-in-the-release.md) で置き換えた。アプリも armhf で出す。CLI を出す決定（2〜6）は変わらない。

## Context

NanoPi NEO（Allwinner H3、Cortex-A7、32bit ARM、メモリ 256〜512MB）で jamuru を動かしたい。リリースの Linux 向け成果物は amd64 の AppImage と deb だけで、NEO では起動しない。

NEO のような小さな機器では、画面を持つアプリ（Tauri。WebKitGTK が要る）は現実的でない。一方、コマンドラインの `jamuru`（`src/main.rs`）は音を ALSA（`cpal`）で扱い、画面を持たない。

## Decision

1. **（ADR-128 で置き換え済み）対象は CLI の `jamuru` だけ。** GUI（Tauri のアプリ）は armhf で出さない。画面に使う WebKitGTK と描画のぶんだけで、NEO のメモリの大半を使う。armhf 向けの AppImage を作る道具（Tauri のバンドラ）の対応も確かめていない。GUI を出す理由が生まれたら、別の決定として扱う。
2. **ターゲットは `armv7-unknown-linux-gnueabihf`。** Ubuntu 22.04 の amd64 のランナーでクロスコンパイルする（Linux のアプリと同じ OS。要求する glibc は同じ 2.35 まで）。クロスコンパイラ（`gcc-arm-linux-gnueabihf`）と armhf の ALSA（`libasound2-dev:armhf`）は `scripts/install-armhf-cross-deps.sh` が入れる。armhf のパッケージは amd64 のミラーに無いので `ports.ubuntu.com` から取り、既存の取得先は amd64 に絞る。
3. **機能は既定のまま（`opus-codec` を付けない）。** アプリも付けずに配っているので、CLI だけ増やさない。付けるとビルドに cmake が要る。armv7 の既定のターゲットには NEON が無いため、そのままでは libopus の NEON の部分（`silk/arm/NSQ_neon.c`）のコンパイルで落ちる。CFLAGS に `-mfpu=neon` を渡せば作れて、qemu で起動した（実機の音は未確認）。ただしその場合、NEON の無い 32bit ARM では動かなくなる。必要になったら、別の決定として扱う。
4. **配布の形は tar.gz の単体バイナリ。** `jamuru-cli-linux-armv7.tar.gz`（`jamuru`・`LICENSE`・`THIRD_PARTY_LICENSES.md`）と、その `.sha256` を同じリリースに添える。armhf の deb は作らない（パッケージの依存と更新の仕組みを持つほどの利用者がいない）。署名はせず、自動更新もしない。`latest.json`（アプリの自動更新の情報）には載せず、Homebrew の cask にも出さない。
5. **公開の前に、qemu-user で `--version` と `--help` が動くことを確かめる**（`scripts/check-cli-runs.sh`）。動かなければリリースを止める。リリースに含めるのは `release.yml` の `build-cli` ジョブで、`release` ジョブはこれを待つ。同じジョブを `ci.yml` にも置き、変更のたびに armhf で作れなくなっていないかを見る（最初は止まっている `build.yml` に置いたため回っておらず、[ADR-127](./ADR-127-arm64-linux-in-the-release.md) で `ci.yml` に移した）。
6. **このジョブは秘密を読まず、environment にも入らず、リポジトリへの書き込み権限（`permissions: contents: read`）もトークンの保持も持たない。** 署名もサーバーの接続先の埋め込みも要らない（CLI はサーバーを `--server` で受け取る）。

## Consequences

- 毎回のリリースに armhf の CLI が含まれる。ビルドが落ちればリリース全体が止まる。
- 要求する glibc は 2.35 まで（実測は 2.34）。Ubuntu 22.04・Debian 12 より古い OS（Debian 11 など）では起動しない。NEO の OS がそれより古いときは、古い OS でビルドし直す別の決定が要る。
- qemu で確かめるのは起動までで、実機での音は確かめない。Cortex-A7 で小さいバッファの処理が間に合うか、音切れが出ないかは、実機で別に測る。
- `armv7-unknown-linux-gnueabihf` は 32bit の ARM 全般（Raspberry Pi の 32bit OS など）でも動く見込みだが、検証したのは qemu での起動までで、実機はない。

## References

- REQ-DIST-010
