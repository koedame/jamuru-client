# ADR-092: ベータのあいだは、チャンネルごとに一番新しい release だけを GitHub に残す

## Status

Accepted。[ADR-057](./ADR-057-beta-channel-rollback.md)（beta-channel を過去の版へ戻す）と [ADR-061](./ADR-061-prune-old-beta-packages.md)（古いベータの dmg・msi・deb だけを消す）を置き換える。

## Context

古い版に脆弱性が含まれていることがある。release が残っていると、それを新しく入れてしまう人が出る。ベータのあいだは、古い版を配り続ける理由より、配らないことの方が大きい。

ADR-061 は、古いベータから dmg・msi・deb だけを消し、自動更新と ADR-057 のロールバックが読む成果物（app.tar.gz・AppImage・setup.exe・署名・`latest.json`）は残した。そのため Windows の setup.exe と Linux の AppImage は、古い版でも Release ページから入れられた。ロールバックが過去の release を要るので、この穴は塞げなかった（ADR-061 の Consequences）。

## Decision

1. **release を公開し、配り先（`beta-channel` の更新と Homebrew Cask の更新）が済んだら、同じチャンネルの古い release を丸ごと（添付ファイルごと）削除する**（`release.yml` の `prune-old-releases` ジョブ、`scripts/prune-old-releases.sh`）。
   - チャンネルは 2 つ: ベータ（`vX.Y.Z-beta.N`）と正式版（`vX.Y.Z`）。残すのは各チャンネルの一番新しい 1 件。正式版が出ても、一番新しいベータは残す。
   - 版の新旧は数で比べる（beta.9 は beta.10 より古い）。公開した版より新しい release は消さない（遅れて終わった run が新しい release を消さない）。
   - `beta-channel` の release は版ではなく、ベータが更新情報を読む場所なので消さない。`beta-channel` の更新情報がまだ指している release も消さない。
   - **git のタグは消さない。** 更新の訓練（ADR-089）は「1 つ前のリリース」をタグからビルドし、Worker や echo も同じタグで出している。
   - 手動起動と定期実行（予行演習）は何も公開しないので、何も消さない。
2. **ロールバックをやめる。** `scripts/publish-beta-channel-rollback.sh` と `FORCE_ROLLBACK` を廃止する（REQ-UPD-019）。戻す先の release が残らないため。悪い版を出したら、直した版を新しいベータとして出す。`beta-channel` は新しい版にしか進まない（ADR-045）ままで、これで足りる。
   - 却下: 直近 N 件を残す。N 件のあいだは古い版が入れられるままで、目的に合わない。
   - 却下: 古い release を draft にして隠す。公開のページからは消えるが、資産は残っていて、消したことにならない。
   - 却下: 更新用の資産と人が落とす資産を別の置き場に分ける（ADR-061 の「未着手」）。ロールバックを残す前提の設計で、残さないなら要らない。

## Consequences

- release の削除は取り消せない。消すのは、新しい release の公開・`beta-channel` の更新・Cask の更新がすべて成功したあとだけで、どれかが失敗したら古い release が残る。残った分は次のリリースの run がまとめて消す。
- 壊れた版を出したときに、前の版へ戻す手段は無い。すでにその版へ更新した端末は、直った版を新しいベータとして出して更新する。
- 古いベータを入れたままの端末は、`beta-channel` が指す最新の版へ更新できる。古い release の URL は埋め込まれていない。
- Update drill が使う「1 つ前のリリース」はタグから取るので、release を消しても動く。
- 開発側の端末に特定の古い版の MSI を入れる操作（`jamjam-beta.sh deploy`）は、`beta-channel` の最新しか入れないままで、変わらない。

## References

- [ADR-045](./ADR-045-beta-self-update.md): `beta-channel`
- [ADR-057](./ADR-057-beta-channel-rollback.md)・[ADR-061](./ADR-061-prune-old-beta-packages.md): 置き換えた決め
- [ADR-089](./ADR-089-update-before-opening-and-keep-the-previous-version.md): 更新の訓練がタグから前の版を取る
- REQ-UPD-020（[requirements.md](../requirements.md)）
