# ADR-123: 配布物のビルドは、画面の依存を lock ファイルのとおりに入れる（`npm ci`）

## Status

Accepted。[ADR-009](./ADR-009-tauri-build-commands.md) の `package.json` の `tauri:build` を置き換える（`tauri:dev` と、プロジェクトルートから実行する決定はそのまま）。

## Context

`cargo tauri build` は `beforeBuildCommand` で `npm run tauri:build` を呼び、そこで `cd ui && npm install && npm run build` していた。
`npm install` は、動かした npm の版に合わせて `ui/package-lock.json` を書き直すことがある。

Linux の配布物を古い glibc で動くように Ubuntu 22.04 のビルド機で作るようにしたところ、そこにある npm 10 が lock ファイルから
`"libc"` の項目を消すようになった（Node 20・npm 10.8.2 で再現した）。第三者ライセンスの表示（`scripts/third-party-licenses.py`）は
lock ファイルを入力として持ち、ビルドのあとに入力が変わっていないかを確かめるので、Release の Linux のビルドがそこで止まり、
ベータ版が 2 回続けて公開されなかった。

止まったのは確認が正しく働いた結果で、問題は「コミットした lock ファイルと違う依存の解決で配布物を作りうる」ことのほうにある。

## Decision

- `tauri:build` は `cd ui && npm ci && npm run build` にする。`npm ci` は lock ファイルを書き換えず、`package.json` と食い違えば止まる
- `tauri:dev` は `npm install` のまま（手元で依存を足しながら開発するため）

## Consequences

- 配布物は、どのビルド機の npm でも、コミットした lock ファイルのとおりの依存で作られる。ライセンスの表示もその依存と一致する
- `package.json` を変えて lock ファイルを作り直し忘れると、ビルドが `npm ci` で止まる（これまでは黙って解決し直していた）
- `npm ci` は毎回 `ui/node_modules` を消して入れ直すので、手元の `cargo tauri build` は少し遅くなる
