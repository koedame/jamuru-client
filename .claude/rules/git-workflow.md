# Git ワークフロールール

## コミットメッセージ

### 言語

- コミットメッセージは英語で記述する

### 形式

- シンプル形式（feat:, fix: などのプレフィックスは使用しない）
- 1行目: 変更内容の要約（50文字以内を目安）
- 2行目: 空行
- 3行目以降: 必要に応じて詳細説明

### 書き方ルール

- 「何を」「なぜ」変更したかを明確に書く
- 1行目は命令形で書く（例: "Add", "Fix", "Change"）
- 曖昧な表現を避ける

❌ 悪い例

```
Fix
Bug fix
Various changes
```

⭕ 良い例

```
Change audio capture buffer size to 20ms

Adjusted frame size to keep latency under 150ms.
```

## コミット単位

- 1つの論理的な変更につき1コミット
- 複数の無関係な変更を1コミットに混ぜない
- ビルドが通る状態でコミットする

## 禁止事項

- 機密情報（APIキー、トークン等）をコミットしない
- 生成ファイル（build/, dist/ 等）をコミットしない（.gitignore で除外）

## Git Push

- `git push` 単体ではなく、リモートとブランチを明示する（例: `git push origin develop`、作業ブランチでは `git push origin <ブランチ名>`）

## ブランチ運用（Git-Flow）

判断は [ADR-047](../../docs-spec/adr/ADR-047-git-flow-branching.md)。正式版を使う人のアプリを壊さないために、開発は develop で進め、main には出した版だけを置く。

| ブランチ | 役割 | 切り元 → 戻し先 |
|---------|------|----------------|
| `main` | 利用者に出した正式版だけ。既定ブランチではない | `release/*`・`hotfix/*` からのマージだけが入る |
| `develop` | 開発の中心。既定ブランチ。PR の向き先 | feature から PR で入る |
| feature（`<内容を表す名前>`） | 1 つの変更 | develop から切り、develop へ PR |
| `release/X.Y.Z` | 版上げなどリリースのための作業だけ（機能は足さない。不具合の修正は可） | develop から切り、main へ、そのあと develop へ |
| `hotfix/<内容>` | 出した正式版の緊急修正 | main から切り、main へ、そのあと develop へ |

- **PR の向き先は既定で develop。** main に向けてよいのは `release/*` と `hotfix/*` だけ。Dependabot の PR も develop（`.github/dependabot.yml` の `target-branch`）
  main と develop へは直接コミットせず、PR（マージコミット）で入れる
- **feature を develop に入れる前**: 最新の develop を feature に取り込み、CI を通す。動作確認が要る変更は、この状態でベータ版を配って確かめ、済んでから develop へマージする
- **リリース**:
  1. develop から `release/X.Y.Z` を切り、`src-tauri/tauri.conf.json` と Cargo.toml の版を X.Y.Z にする（`vX.Y.Z` のタグは、この版と同じでないと更新情報を作る段階で失敗する）
  2. `release/X.Y.Z` を main にマージし、main の先頭に `vX.Y.Z` のタグを打つ（タグのプッシュで公開される）
  3. main を develop に戻しマージし、develop の版を次の版に上げる（`X.Y.Z-beta.N` は `X.Y.Z` より古い版なので、出した版のままではベータ版が更新に見えない）
  4. `release/X.Y.Z` を消す
- **ベータ版**: タグ `vX.Y.Z-beta.N` をコミットに打つとリリースのワークフローが公開する。ブランチへのプッシュでは公開されない。
  打つブランチは決めない（develop が基本。feature の動作確認は feature ブランチ。main には打たない想定だが禁止はしない）。
  N は、その X.Y.Z で既に出た最大の N に 1 を足す（小さい N を打つと、入っているベータ版が新しい版と見なさない）
- **タグは対になるサーバー側のリポジトリと同じ名前を、同じときに打つ。** ベータ版でも正式版でも、どちらかの変更しか無くても両方に打つ
  （変更の無い側は、その時点の先頭に打つ。1 つのコミットに複数のタグが付いてよい）。
  ある版がどのコミットどうしを前提にしていたかを、タグで追えるようにするため
