---
sidebar_label: "ADR-061: Prune Old Beta Packages"
sidebar_position: 61
---

# ADR-061: ベータ版が新しく出たら、古いベータ版の dmg・msi・deb だけを消す。更新とロールバックが使う成果物は消さない

## Status

Accepted

## Context

ベータ版のタグ `vX.Y.Z-beta.N` を打つたびに、GitHub の Release に 5〜7 個のファイルが残る（`release.yml` の `release` ジョブ）。
検証が終わったベータ版には使い道が無く、Release ページから誰でも古いベータ版をダウンロードしてインストールできてしまう。

ただし、同じ Release の中の一部のファイルは検証が終わっても手放せない。

- **`*.app.tar.gz`（macOS）・`*.AppImage`（Linux）・`*-setup.exe`（Windows）とそれぞれの署名**は、インストール済みのベータ版が
  自分を新しい版に入れ替えるのに使う（[ADR-045](./ADR-045-beta-self-update.md)、REQ-UPD-004・REQ-UPD-009）。
- **`latest.json`** は、悪いベータ版が出たとき `scripts/publish-beta-channel-rollback.sh` がここから読み直して
  beta-channel を過去の版へ意図的に戻す（[ADR-057](./ADR-057-beta-channel-rollback.md)、REQ-UPD-019）。**戻す先の Release が
  無いと戻せない**（ADR-057「Consequences」）。

これに対して、**`.dmg`（macOS）・`.msi`（Windows）・`.deb`（Linux）は、どれも自分自身を入れ替えられない配布形態**（REQ-UPD-004）。
アプリの自動更新はこの 3 つを一度も読みに行かない。手で入れ替えるか、`.msi` は管理者権限で・`.deb` はパッケージ管理で個別に
更新してもらう前提（`docs-site/docs/getting-started/installation.md`「自動更新」）。つまりこの 3 つは、**新しいベータ版が出た
瞬間に「Release ページから人が直接落として入れる」以外の使い道が無くなる**。

## Decision

1. **ベータ版のタグを公開したら（`prune-old-beta-packages` ジョブ）、それより古いベータ版の Release から `.dmg`・`.msi`・`.deb`
   （とその署名。あれば）だけを削除する**（`scripts/prune-beta-packages.sh`）。
   - 対象は `vX.Y.Z-beta.N` の形のタグだけ。正式版の Release・`beta-channel` は対象にしない。
   - 消さずに残すのは、自動更新とロールバックが読む成果物（`app.tar.gz`・`AppImage`・`setup.exe`・それぞれの署名）と
     `latest.json`。**Release 自体・タグも消さない**（ロールバック先として残す必要がある。ADR-057）。
   - いま公開したタグ自身は対象から除く（最新のベータ版は、動作確認用に丸ごと残す）。
2. **却下: 古い Release を丸ごと削除する。** 一番単純だが、`latest.json` も一緒に消えるため、そのベータ版へは二度と
   `publish-beta-channel-rollback.sh` で戻せなくなる。
3. **却下: 古い Release を draft にして隠す。** 公開の Release ページからは消えるが、ドラフトの asset は
   認証したアクセスでしか読めなくなる。ロールバックした端末は、埋め込まれた素の URL（無認証の HTTP）で
   `app.tar.gz`・`AppImage`・`setup.exe` を取りに行くため、ここが読めないと肝心のロールバックが機能しない。
4. **却下: 直近 N 件のベータ版だけ残す。** 「最新のベータ版は残す」という要求はどの N でも満たせるが、
   N を超えた時点のロールバックはやはりできなくなる点は変わらず、単に猶予が延びるだけで問題の形は変わらない。
   1 件だけ残す方が、何が起きるかを説明しやすい。

## Consequences

- **Windows と Linux は、この対応だけでは「古いベータ版をインストールできない」状態にならない。** `-setup.exe`
  （Windows の既定のインストーラ）と `.AppImage`（Linux の既定の配布形態）は、どちらも自動更新とロールバックが使う
  成果物そのものなので、消せない。macOS だけは `.dmg` を消すことで、Release ページからの手動インストールを止められる
  （既定の Homebrew 配布はもともと最新の cask しか指さないので影響しない）。**Windows・Linux まで完全に閉じるには、
  「更新に使う成果物」と「人が手で落とす成果物」を別の置き場に分ける設計変更が要る**（未着手。要るかどうかは
  ロールバックの安全性とどちらを優先するかの判断が要る）。
- ロールバックできる先は、常に「直近で消されていない Release」に限られる。今の対応では**1 つ前のベータ版までしか
  戻せない**（2 つ前は、1 つ前が出た時点で当時のロールバック用ファイルは残るが、Release 自体は削除していないので
  実際にはどこまでも遡れる。消しているのはインストール専用ファイルだけ）。
- `scripts/prune-beta-packages.sh` は削除に `gh release delete-asset --yes` を使う。対象を間違えると復元できない
  （Release の asset 削除は取り消せない）ため、削除条件は拡張子の完全一致に絞り、対象の絞り込みを広げない。

## References

- [ADR-045](./ADR-045-beta-self-update.md): ベータ版の自動更新。`.dmg`・`.msi`・`.deb` が読まれない根拠（REQ-UPD-004）
- [ADR-057](./ADR-057-beta-channel-rollback.md): 過去の Release が残っている前提のロールバック
- REQ-UPD-020（[requirements.md](../requirements.md)）
