---
sidebar_label: "ADR-091: Release Credentials in Environments"
sidebar_position: 91
---

# ADR-091: 署名の証明書・鍵と tap のトークンは environment のシークレットに置き、決めた ref にしか開かない

## Status

Accepted。[ADR-030](./ADR-030-signaling-url-by-build-profile.md) が「リポジトリの secret」と書いた置き場のうち、署名と tap の更新に使うものを environment に移す。

## Context

リポジトリのシークレットは、このリポジトリに push されたどのブランチのワークフローからでも `secrets.<名前>` で読める。署名の証明書・鍵と tap へ書き込むトークンは、`release.yml` の 2 つのジョブしか使わないのに、そのように置かれていた。

## Decision

1. **署名と公開の資格情報は environment のシークレットに置く。**

   | environment | シークレット | 読むジョブ |
   |---|---|---|
   | `desktop-signing` | `APPLE_CERTIFICATE` `APPLE_CERTIFICATE_PASSWORD` `TAURI_SIGNING_PRIVATE_KEY` `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `release.yml` の `build` |
   | `tap-publish` | `TAP_GITHUB_TOKEN` | `release.yml` の `update-cask` |

2. **ジョブが名指しする environment は 1 つだけ。** 署名と tap の更新は別のジョブなので、互いの資格情報を読めない。
3. **environment は ref で絞る。** `desktop-signing` は `develop`・`main` のブランチと `v*` のタグ、`tap-publish` は `main` のブランチと `v*` のタグだけ。ベータ版のタグ（`vX.Y.Z-beta.N`）は feature ブランチにも打つので、ブランチでなくタグで開ける。`develop` を足したのは、週 1 回の予行演習（`schedule`）が develop で走り、署名の鍵がないとビルドが通らず、ビルドのキャッシュが作れないため。ほかのブランチからの手動起動（`workflow_dispatch`）は、署名の environment に入れないので予行演習も走らない。
4. **同じ名前のリポジトリのシークレットは消す。** environment のシークレットは、その environment を名指しするジョブにだけ効き、リポジトリのシークレットを隠す。消さなければ、ほかのジョブからは読めたままになる。
5. **`JAMJAM_SERVER_URL` と `JAMJAM_SERVER_URL_BETA` はリポジトリのシークレットのまま。** 値は配布するアプリに入る本番とステージングのサーバーの URL で、利用者の手元に渡るもの。`build.yml` が pull request でも読む（アプリに入るか検査する）ので、environment に移すとその検査が動かなくなる。

## Consequences

- 署名と公開の資格情報は、`develop`・`main` とリリースのタグ以外の ref のワークフローには渡らない。
- 別のブランチ・fork でリリースを組むには、その ref を environment に足す。
- `tests/distribution_config_test.rs` が、これらのシークレットを読むジョブが決めた environment を 1 つだけ名指ししていることを検証する。environment の中身と ref の絞り込みは GitHub 側の設定で、テストでは見られない。

## References

- REQ-DIST-009
