<!-- このドキュメントは実装の正です。変更時は実装も同期すること -->

# ADR-029: 配布前に固めるアプリ設定（識別子・CSP・マイク使用の説明）

## Status

Accepted

## Context

`src-tauri/tauri.conf.json` に、開発中は症状が出ないが、配布すると利用者の手元で壊れる、または後から変えられない設定が 3 つあった。

| 設定 | 状態 | 配布後に起きること |
|------|------|------------------|
| macOS のマイク使用の説明 | `NSMicrophoneUsageDescription` を宣言した Info.plist が無い | macOS は説明を持たないアプリがマイクに触れた瞬間にプロセスを終了させる。`cargo tauri dev` はターミナルの権限で動くため開発中は再現しない |
| アプリ識別子 | `com.jamjam.app`（自分の管理しないドメインの逆順） | 識別子は webview の保存領域と OS への登録（バンドル ID、招待リンクのスキームの登録先）に使われる。配布後に変えると利用者の webview データが引き継がれない |
| CSP | `"csp": null`、`withGlobalTauri: true` | 画面に他の参加者の文字列（チャット・表示名）が出る。スクリプトが混ざると Tauri のコマンドをすべて呼べる |

## Decision

### 1. マイク使用の説明を `src-tauri/Info.plist` で宣言する

Tauri 2 は `src-tauri/Info.plist` を macOS のバンドルの Info.plist にマージする。ここに `NSMicrophoneUsageDescription` だけを置く。説明文は英語とする（macOS の許可ダイアログにそのまま表示される）。

### 2. アプリ識別子を `me.koeda.jamjam` にする

自分が管理するドメインの逆順にそろえる。他者と衝突しない。

識別子に依存するのは webview の保存データだけである。`config.toml` と端末識別子の保存先は `directories::ProjectDirs::from("", "", "jamjam")` で決まり、識別子には依存しない（[architecture.md §13](../architecture.md)）。

| データ | 保存先 | 識別子変更の影響 |
|--------|--------|----------------|
| `config.toml` | アプリ名 `jamjam` のディレクトリ | なし |
| 端末識別子 | 同上 | なし |
| 表示言語の選択（`localStorage`） | 識別子ごとの webview 領域 | 初期状態に戻る（OS の言語から再検出） |
| 最近使った絵文字（`localStorage`） | 同上 | 空に戻る |

まだ配布していないため、影響を受けるのは開発者の環境だけである。移行コードは書かない。失われるのは再選択で戻せる 2 つの UI 状態で、移行コードは一度も配布されない版からの移行のために製品へ残り続けるためである。開発者向けの扱いは README に書く。

### 3. CSP を有効にし、グローバル API を公開しない

```json
"csp": {
  "default-src": "'self'",
  "script-src": "'self'",
  "style-src": "'self'",
  "font-src": "'self'",
  "img-src": "'self'",
  "connect-src": "'self' ipc: http://ipc.localhost",
  "object-src": "'none'",
  "base-uri": "'none'",
  "form-action": "'none'"
}
```

- webview はネットワークに出ない。シグナリングも音声も Rust 側で行う。したがってリモートのオリジンを 1 つも許可しない
- `connect-src` の `ipc:` / `http://ipc.localhost` は Tauri の IPC の経路である（前者が macOS / Linux、後者が Windows）。無いとすべてのコマンドが失敗する
- `unsafe-inline` / `unsafe-eval` は許可しない。本番ビルドの Vite は CSS をファイルに出力し、React の `style` 属性は CSSOM 経由で設定されるため CSP の対象外である。Tauri は自分が注入するスクリプト・スタイルに nonce / hash を付与する
- GUI E2E の制御チャネル（[ADR-025](./ADR-025-gui-e2e-control-channel.md)）はネイティブの `eval_with_callback` で JS を評価し、ページの CSP を経由しない。追加の許可は不要である
- `withGlobalTauri` を `false` にする。UI は `@tauri-apps/api` をモジュールとして import しており、`window.__TAURI__` を参照するコードは無い

Tauri 2.11 はこの CSP を、同梱資産を返す `tauri://` プロトコルの応答ヘッダとして付ける（HTML の `<meta>` には出ない）。デスクトップの `cargo tauri dev` は Vite の開発サーバーを直接読み込むため、CSP は付かない（開発サーバーを経由させるのはモバイルのみ）。CSP 下の挙動を確かめるときは、同梱資産を読み込むビルド（`cargo build --manifest-path src-tauri/Cargo.toml` の既定 feature、または `cargo tauri build`）を使う。

### 4. 設定をテストで固定する

`tests/distribution_config_test.rs` が `tauri.conf.json` と `Info.plist` を読み、上の 3 点を `cargo test` のたびに検証する（REQ-DIST-001〜004）。どれも開発中に気づく経路が無いため、設定が戻ったことを検出できるのはこの層だけである。

### 5. 検証（Linux、WebKitGTK）

| 確認したこと | 方法 | 結果 |
|-------------|------|------|
| CSP が効く | 同梱の `index.html` に `<img onerror="...">` を差し込んでビルドし、ハンドラが実行されたかを制御チャネルで読む | 本変更前は実行され、本変更後は実行されない |
| CSP 下でも画面が変わらない | 同じ条件で起動した接続画面のスクリーンショットを本変更の前後で比較 | 完全一致 |
| GUI E2E（ADR-025）が壊れない | `cargo test --features gui --test gui` を本変更の前後で実行 | 前後とも 19 件成功。失敗 5 件はループバック音声ドライバを使うシナリオで、前後で同一 |
| 識別子が保存先に効く | 起動後の `$HOME` を確認 | webview のデータは `~/.local/share/me.koeda.jamjam`、`config.toml` は `~/.config/jamjam` |

## Consequences

### 良い影響

- macOS の配布物がマイクに触れても終了しない（宣言上。実機での確認は別途必要）
- 識別子を最初のリリース前に確定したので、以降の版で利用者の webview データが引き継がれる
- 表示文字列にスクリプトが混ざっても実行されない

### 悪い影響・残る作業

- REQ-DIST-001 は宣言の存在までしか自動で検証できない。macOS 実機で許可ダイアログに説明が表示されることを、インストール版の確認時に見る必要がある
- 開発者の手元の表示言語・最近使った絵文字が一度だけ初期状態に戻る
- 将来 webview からリモートの資産（画像・フォント）を読むときは、CSP とテストの両方を更新する

## 関連

- [ADR-009: Tauri ビルドコマンド](./ADR-009-tauri-build-commands.md)
- [ADR-025: GUI E2E 制御チャネル](./ADR-025-gui-e2e-control-channel.md)
- [architecture.md §13 設定ファイル](../architecture.md)
