# ADR-063: 第三者ライセンスの表示は、実際の依存から生成して検査する

## Status

Accepted

## Context

`THIRD_PARTY_LICENSES.md` と、インストーラーに同梱する `src-tauri/resources/LICENSES.txt` は手書きの表だった。
Rust のクレートは 53 行、実際の依存は 628 パッケージで、MPL-2.0 の 5 件・直接の依存の多くが載っていなかった。
一方で、依存に無い `ringbuf` が載っていた。Apache-2.0 と OFL-1.1 は本文の代わりに URL だけだった。
MIT・Apache-2.0・BSD・MPL-2.0 は、配るときに著作権表示とライセンス本文（MPL-2.0 はソースの入手先も）を添えるのが条件なので、
漏れているぶんは条件を満たしていなかった。「GPL・LGPL は導入禁止」も人の目だけで、検査が無かった。

## Decision

- 表示は**ロックファイルから生成する**。`scripts/third-party-licenses.py` が cargo-about（Rust のクレート。ルートと `src-tauri`、
  全機能、Linux・Windows・macOS）と `ui/package-lock.json`（UI が配る npm パッケージ。dev 専用を除く）を読み、
  各依存のクレート・パッケージ自身の著作権表示とライセンス本文、MPL-2.0 のソースの入手先を `THIRD_PARTY_LICENSES.md` と
  `LICENSES.txt` に書く。ソースに写した Lucide のアイコンは `packaging/third-party/` の本文を足す。生成物は手で編集しない
- **許可するライセンスは `deny.toml` の一覧だけ**（`about.toml` の `accepted` と同じ。食い違えばスクリプトが落ちる）。
  GPL・AGPL・LGPL 単独・ライセンス不明の依存が入れば、CI の `licenses` ジョブ（`cargo deny check licenses`）が止める。
  `MIT OR Apache-2.0 OR LGPL-2.1-or-later` のように許可されたものを選べる依存は通る
- `--check` は cargo-about を回さず（数分かかる）、生成物と、その入力（ロックファイル・`about.toml`・`deny.toml`・
  `packaging/third-party/`・スクリプト）のハッシュを突き合わせる。入力が変わったのに作り直していない・生成物を手で編集した、のどちらも落ちる。
  `cargo test`（`tests/third_party_licenses_test.rs`）・CI・リリースが回す。依存を更新する PR は、
  作り直した生成物を同じ PR に含める。Dependabot の PR は、`.github/workflows/dependabot-licenses.yml` が作り直して PR のブランチに 1 コミット足す
  （Dependabot の `pull_request` の実行は書き込めないので `pull_request_target` で動かし、Dependabot 自身の同一リポジトリの PR・
  マニフェストとロックファイルだけを変える PR・トークンを渡さない生成の段、の 3 つで絞る。push が起こした CI は承認待ちで止まるので、ワークフローが承認する）
- **Linux の AppImage** には、ビルドした環境のシステムのライブラリ（GTK・WebKitGTK・GLib・GStreamer など。LGPL が多い）が同梱される。
  どれが入るかはビルドの環境で変わる（0.1.0 と beta.48 の AppImage でも違っていた）ので、一覧は手で持たず、
  リリースのビルドが AppImage から作る（`scripts/appimage-bundled-libraries.py`。`dpkg` で引き、著作権ファイル・ライセンス本文・
  ソースの入手先を `jamjam-linux-bundled-libraries.txt` にして AppImage と同じリリースに置く）。パッケージに引けないライブラリがあればリリースを止める

## Consequences

- 依存を上げるたびに `scripts/third-party-licenses.py` を回して生成物をコミットする（約 2 分。cargo-about が要る）。Dependabot の PR では自動で回る
- Dependabot は他人が足したコミットのある PR を自動では rebase しなくなる。遅れたら `@dependabot recreate` で作り直す（ワークフローがまた生成物を足す）
- 生成物は約 700KB で、インストーラーが同じ大きさだけ増える
- AppImage に LGPL のライブラリを同梱することと、LICENSE の「改変・再配布・自己ビルドの禁止」の関係は、この ADR では決めない。
  配り方（AppImage を続けるか、`.deb` だけにするか）は別に判断する
