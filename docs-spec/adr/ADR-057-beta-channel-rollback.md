---
sidebar_label: "ADR-057: Beta Channel Rollback"
sidebar_position: 57
---

# ADR-057: 悪いベータ版が出たら、beta-channel を過去の版へ意図的に戻せるようにする

## Status

Accepted。[ADR-045](./ADR-045-beta-self-update.md) の 2 節が持つ「より新しい版のときだけ置き換える」制約は変えない。この ADR はその制約を、明示的な操作のときだけ迂回する手段を足す。

## Context

ADR-045 でベータ版も自動更新するようになった。版ごとの固まり・クラッシュの件数を数える仕組み（別 ADR）で悪い版に気づいても、`scripts/publish-beta-channel.sh` は「より新しい版のときだけ置き換える」設計（遅れて終わった古い run が置き場を巻き戻さないための安全装置）になっており、悪い版を配り続けている間、前の版へ戻す手段が無かった。

この安全装置は意図的な設計であり、外すのではなく、**明示的なロールバック操作のときだけ迂回できるようにする**。

## Decision

1. **`scripts/publish-beta-channel.sh` に `FORCE_ROLLBACK=1` を足す。** 設定されていると「より新しい版のときだけ置き換える」チェックを飛ばす。release のワークフローはこの変数を設定しないので、通常の公開経路は今までどおり安全装置がかかる。
2. **`scripts/publish-beta-channel-rollback.sh <release-tag>` を新設する。** 戻す先を release のタグ 1 つで指定する。中身は次の 2 手順だけ:
   - 指定した release から、`latest.json`（`scripts/make-update-manifest.sh` がその release を作った時点で書いた、その版向けの署名済みの更新情報）を読む。
   - `FORCE_ROLLBACK=1` を付けて `publish-beta-channel.sh` にそのまま渡す。
   - 却下: 対象の release の成果物・署名から `latest.json` を作り直す。release は公開時点で `make-update-manifest.sh` の全チェック（版の一致・署名の版・成果物の欠落）を通った `latest.json` を既に資産として持っている。作り直すと同じチェックをもう一度実装することになり、かつ作り直したものが元と食い違う不具合を生みうる。既にある正しいものを読み直す方が単純で安全。
   - 却下: 版番号（`0.1.0-9` 等）を直接指定する。release のタグは Git の tag・GitHub Actions の run・成果物と 1 対 1 で結び付くが、版番号だけでは「どの run のどの成果物か」が本文から追えない。
3. **指定した release に `latest.json` が無い（release 自体が無い、または ADR-041 より前に出した release で更新情報を持たない）ときは、何も置き換えずに失敗する。** 空の `latest.json` や壊れた内容で beta-channel を上書きしない。
4. **この操作自体（実際に beta-channel を書き換える）は、悪い版が実際に出て戻す判断をしたときに手で行う。** 自動では走らせない（版ごとの固まり・クラッシュの件数を数えてしきい値を超えたときに自動でロールバックする、という判断は別の ADR で扱う）。

## Consequences

- **ロールバックは、まだその版を取得していない端末を止めるだけ**で、既にその版へ更新済みの端末を戻すものではない。アプリの更新は「今より新しい版」だけを入れる（ADR-041）ので、beta-channel を古い版に戻しても、既に新しい版で動いている端末は自動では戻らない。既に取得してしまった端末には、直った版が出るまで `debug.update_apply`（ADR-044）等の遠隔操作で個別に対応する。
- ロールバック先の release がまだ存在する前提になる。release を消してしまうと戻せない。
- `FORCE_ROLLBACK` は環境変数なので、`publish-beta-channel.sh` を直接呼ぶ経路（release ワークフロー）に紛れ込ませない運用が要る。ワークフロー側で明示的に設定しない限り効かないため、通常の公開は影響を受けない。

## References

- [ADR-041](./ADR-041-self-update.md): 自動更新の全体。署名済みの `latest.json` の作り方（`make-update-manifest.sh`）
- [ADR-045](./ADR-045-beta-self-update.md): beta-channel の「より新しい版のときだけ置き換える」制約
- [ADR-044](./ADR-044-portals-and-permissions.md): 個々の端末を戻す `debug.update_apply`
- REQ-UPD-019（[requirements.md](../requirements.md)）

## Runbook（悪いベータ版に気づいたとき）

1. 戻す先の release のタグを決める（悪い版の 1 つ前の、健全だった release）。
2. `gh auth login` 済みの環境で実行する:
   ```sh
   scripts/publish-beta-channel-rollback.sh v0.1.0-beta.16
   ```
3. 出力に `beta-channel now has <版>` が出れば完了。以後、新しく起動した・まだ更新していないベータ版の端末はこの版を取得する。
4. 既にこの版へ更新済みの端末には、`debug.update_apply` 等の遠隔操作で個別に直った版を入れる（このスクリプトでは戻らない。「Consequences」参照）。
5. 直った release を出したら、通常どおり release のワークフローが beta-channel を新しい版へ進める（このスクリプトの出番はここで終わる)。
