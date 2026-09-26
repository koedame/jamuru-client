---
sidebar_label: "ADR-045: Beta Self Update"
sidebar_position: 45
---

# ADR-045: ベータ版も自動更新する。ベータ版は固定タグの Release から更新情報を読み、ベータごとに違う版としてビルドする

## Status

Accepted。[ADR-041](./ADR-041-self-update.md) の 4 節「配るのは正式版だけ」（と、ベータ版どうしは自動更新しない決め）を置き換える。

## Context

ADR-041 は、ベータ版のアプリも正式版と同じ `releases/latest/download/latest.json` を見る作りにした。GitHub の `releases/latest` は pre-release を含まない。正式版がまだ 1 つも無いあいだ、そこは **404 を返し続ける**。ベータ版は起動のたびに次を出し、一度も更新できていなかった。

```
ERROR [tauri_plugin_updater::updater] update endpoint did not respond with a successful status code
WARN  [jamjam_app_lib::updater] Self-update did not finish: Could not fetch a valid release JSON from the remote
```

ベータ版は開発者が遠隔から操作して検証する（ADR-044）。遠隔地に置いた端末は、`debug.update_apply` で新しいベータ版に入れ替わることに頼る。更新が働かないと、端末を触りに行くまで古い版のまま検証することになる。

もう 1 つ、更新が成り立たない理由があった。ベータ版のアプリは `X.Y.Z` の版としてビルドされる（`tauri.conf.json` の版）。次のベータ版も同じ `X.Y.Z` なので、更新の部品から見ると「同じ版」で、新しい版として案内されない。

## Decision

1. **ベータ版のビルドは、更新情報を固定タグの Release `beta-channel` の `latest.json` から読む**（`https://github.com/koedame/jamjam-client/releases/download/beta-channel/latest.json`）。ベータ版のビルドにだけ `src-tauri/tauri.updater.beta.conf.json` を足して渡す。公開鍵・署名の検査は正式版のビルドと同じ設定（`tauri.updater.conf.json`）から来て、ここでは置き換えない。正式版のビルドが読む場所は変えない。
   - 却下: 正式版が出るまで待つ。ベータ版の更新は今要る。
   - 却下: Homebrew の cask だけに任せる（ADR-041 の当初の決め）。`brew upgrade` は人が打つもので、遠隔地の端末には届かない。cask はこれまでどおり更新される。
   - 却下: ベータ版ごとに Release へ `latest.json` を置き、`releases/latest` を pre-release に向ける。GitHub は pre-release を `latest` にしない。
2. **`beta-channel` には、ベータ版と正式版の更新情報を置く。** 置き換えるのは、置く版が今の版より新しいときだけ（`scripts/publish-beta-channel.sh`）。
   - ベータ版は、同じ版の正式版（`0.1.0-17` に対する `0.1.0`）が出るとそこに移る。正式版は、そのあとのベータ版（次の版の `0.2.0-1`）が出るまでの置き場になる。
   - 版が新しいときだけ置き換えるのは、遅れて終わった古い run が置き場を巻き戻さないため。同じ版の正式版が出たあとも main へのマージでは `0.1.0-N` が出続けるが、`0.1.0` の方が新しいので置き場は動かない。版を上げると `0.2.0-1` から始まって動く。
   - `beta-channel` は pre-release として作る。"Latest" には載らず、`releases/latest` にも影響しない。タグ名が `v` で始まらないので、Release のワークフローは動かない。
3. **ベータ版のタグ `vX.Y.Z-beta.N` のアプリは、`X.Y.Z-N` の版としてビルドする**（`scripts/build-version.sh`）。ビルド時の `--config` で `tauri.conf.json` の版を上書きする。署名も更新情報もこの版を指し、`make-update-manifest.sh` が署名の版を検査する。
   - `-N` は数字だけにする。Windows の MSI は pre-release に 65535 以下の数字しか受け付けない。`-beta.N` のままでは MSI が組めない。
   - `X.Y.Z-N` は semver で `X.Y.Z` より古く、`-N` は数として比べるので、`-9` より `-17` が新しい。
   - `-beta.N` の形でない pre-release のタグ（`-rc.1` など）は組まない。数字の付け方が決まっておらず、ベータ版との新旧が付かないため。
4. **止め方や更新のふるまいは正式版と同じ。** 起動の 20 秒後と 6 時間おき、セッション中は入れない、`auto_update = false` で止まる（ADR-041 の 1・2・6 節）。ベータ版を更新するのは `debug.update_apply` でもよい（ADR-044）。

## Consequences

- **この ADR より前に配ったベータ版（beta.17 まで）は、埋め込まれた取得先が `releases/latest` のまま更新されない。** 一度だけ手で入れ替える（`brew upgrade --cask jamjam@beta`、または DMG・AppImage を入れ直す）。入れ替えたあとから自動で更新する。
- **`beta-channel` の取得先は、出荷したベータ版に埋め込まれる。** 変えると、変える前のベータ版は更新できなくなる。ADR-041 の署名鍵と同じ扱いで、変えるなら手で入れ替えてもらう前提にする。
- ベータ版の版は、`tauri.conf.json` の `X.Y.Z` に `-N`（Actions の実行番号）が付く。アプリの画面・ログ・`debug.info` に出る版が `0.1.0-17` になる。Homebrew の cask の版（`0.1.0-beta.17`）とは書き方が違う。タグ・cask・Release の名前は変えない。
- `beta-channel` の置き換えは、複数の run が同時に終わると、比べたあとで別の run が置く間がある（数秒）。古い版が残っても、次のベータ版か正式版の run が置き直す。

## References

- [ADR-041](./ADR-041-self-update.md): 自動更新の全体。4 節を本 ADR が置き換えた
- [ADR-044](./ADR-044-portals-and-permissions.md): ベータ版だけが持つ遠隔操作。`debug.update_apply`
- REQ-UPD-012〜014（[requirements.md](../requirements.md)）
