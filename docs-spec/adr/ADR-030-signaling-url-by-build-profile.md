---
sidebar_label: "ADR-030: Server URL by Build Profile"
sidebar_position: 30
---

<!-- このドキュメントは実装の正です。変更時は実装も同期すること -->

# ADR-030: サーバーの URL をビルドの種別で決めて 1 か所に置き、シグナリングの接続先はサーバーに問い合わせる

## Status

Accepted

## Context

リリース用ワークフロー（`.github/workflows/release.yml`）でビルドしたアプリが `ws://localhost:17890` に接続していた。利用者の手元にシグナリングサーバーは無いので、ルームを作ることも参加することもできない。

接続先の既定値が 2 か所にあり、どちらもリリースで localhost になっていた。

| 場所 | 決め方 | リリースビルドでの値 |
|------|--------|--------------------|
| 接続画面（`ui/src/screens/MainScreen.tsx`） | `import.meta.env.VITE_SIGNALING_SERVER`、無ければ `ws://localhost:17890` | `release.yml` も `tauri.conf.json` の `beforeBuildCommand` も値を渡さず、`.env.*` は Git 管理外なので localhost |
| 自己診断（`src-tauri/src/diagnostics.rs`） | `config.toml` の `signaling_server_url`、無ければ `ws://localhost:17890` | localhost |

2 か所は互いに独立していた。接続画面は `config.toml` の `signaling_server_url` を読まず、自己診断は `VITE_SIGNALING_SERVER` を読まない。どちらか一方だけを直すと、診断が接続画面と別のサーバーを測る。

本番のシグナリングサーバーは別に運用している。その場所はソースに書かない（サーバー側の事情で決まり、移すこともある）。

### 改訂（2026-09）

当初はシグナリングの WebSocket の URL そのもの（`wss://`）をビルド時に渡していた。これだとシグナリングを別の場所へ移すたびに、アプリを出し直さなければならない。そこでアプリが持つのをサーバーの URL（`https://`）だけにし、シグナリングの接続先は繋ぐたびにサーバーへ問い合わせる形に改めた（Decision 6）。以下の Decision は改訂後の形で書いている。

## Decision

### 1. サーバーの URL はコアライブラリの `jamjam::config` に 1 つだけ置く

```rust
pub const DEV_SERVER_URL: &str = "http://localhost:17890";
pub const RELEASE_SERVER_URL: Option<&str> = option_env!("JAMJAM_SERVER_URL");
pub const DEFAULT_SERVER_URL: &str = if cfg!(debug_assertions) {
    DEV_SERVER_URL
} else {
    match RELEASE_SERVER_URL {
        Some(url) => url,
        None => "",
    }
};
```

`AppConfig::effective_server_url()` が「`config.toml` の `server_url`、無ければ `DEFAULT_SERVER_URL`」を返す。接続（`signaling_connect`）と自己診断は、どちらも Tauri 側の `ConfigState::server_url()` を通してこれを読む。

`signaling_connect` は URL を引数に取らない。UI が接続先を持つ経路を無くすためである。UI・`src-tauri/src` に `ws://` / `wss://` / `VITE_SIGNALING_SERVER` と、例示用以外のホストの `http://` / `https://` を書かないことを `tests/distribution_config_test.rs` が検査する。

### 2. ビルドの種別（`debug_assertions`）で既定値を切り替える

| ビルド | `debug_assertions` | 既定のサーバー |
|--------|-------------------|-------------|
| `cargo tauri dev`、GUI E2E（`cargo build --features e2e-control`） | 有効 | `http://localhost:17890` |
| `cargo tauri build`（`build.yml` / `release.yml`） | 無効 | ビルド時の `JAMJAM_SERVER_URL` |

`const` の評価で片方だけを選ぶので、リリースビルドのバイナリには開発用の URL が入らない。

### 3. 本番のサーバーはソースに書かず、リリースを組むときに渡す

本番のサーバーはリポジトリに置かない。リリースを組む人（`build.yml` / `release.yml` ではリポジトリの secret `JAMJAM_SERVER_URL`）がビルド時に環境変数 `JAMJAM_SERVER_URL` で渡し、コアライブラリが `option_env!` で取り込む。

`release.yml` はタグが `-` を含む（ベータ）ビルドだけ、渡す secret を `JAMJAM_SERVER_URL_BETA`（ステージングのサーバー）に切り替える。ベータは検証用であり、本番の利用者・本番のデータに影響させないためである（Decision 7）。

環境変数は渡し忘れると黙って別の接続先になりうる（当初の欠陥は `VITE_SIGNALING_SERVER` の渡し忘れで localhost になった）。そこでアプリのビルドスクリプト（`src-tauri/build.rs`）が、リリースのビルドで `JAMJAM_SERVER_URL` が無いとき、`https://` でないとき、ループバックのホストを指すときにビルドを失敗させる。渡し忘れたリリースは作られない。CLI とライブラリのリリースビルドは既定のサーバーを使わないので、渡さなくてもビルドできる。

`tests/distribution_config_test.rs` は、コアライブラリのソースに例示用（`example.com`）以外の `https://` / `wss://` の URL が無いことを検査する。本番のサーバーがソースに書き戻されるのを防ぐ。

### 4. 別のサーバーに繋ぐときは `config.toml` で上書きする

テストサーバーや手元の別ポートに繋ぐときは、`config.toml` に `server_url = "https://..."` を書く。接続画面と自己診断の両方に効く。これに伴い、ビルド時にモードを切り替えるだけだった `package.json` の `tauri:build:test` / `tauri:build:prod` を削除した。

### 5. 成果物を検査する

| 検査 | いつ | 何を見るか |
|------|------|-----------|
| `src-tauri/build.rs`（REQ-DIST-005） | アプリのリリースビルドのたび | `JAMJAM_SERVER_URL` があり、`https://` でループバックでないこと |
| `tests/distribution_config_test.rs`（REQ-DIST-005） | `cargo test` のたび | 開発 URL が `localhost` の `17890` 番であること、コアライブラリに本番のサーバーが書かれていないこと、UI と Tauri コマンドにサーバーの URL が書かれていないこと |
| `scripts/check-release-server-url.sh` | `build.yml` / `release.yml` の `cargo tauri build` の直後 | アプリのバイナリにビルド時の `JAMJAM_SERVER_URL` が入っていること、バイナリと `ui/dist` にループバックの `http://` / `https://` / `ws://` / `wss://` が無いこと（Tauri がどのビルドにも埋め込む開発用の画面の URL、`tauri.conf.json` の `devUrl` だけは除く。リリースは同梱の画面を読むので使わない）。公開のワークフローのログに残るので、接続先そのものは出力しない |

前者は安く常に回る。後者は配布物そのものを見る。バイナリに本番 URL が無ければ失敗させるのは、デバッグビルドや別のファイルを渡したときに検査が素通りしないためである。

### 6. シグナリングの接続先は、繋ぐたびにサーバーへ問い合わせる

アプリはシグナリングの接続先を持たない。`SignalingClient::connect` が、サーバーの URL に `GET /api/v1/signaling` を問い合わせ、返ってきた `{"url": "wss://..."}` の WebSocket に繋ぐ（`src/network/discovery.rs`）。シグナリングを別の場所へ移しても、サーバーが返す URL を変えればアプリを出し直さずに済む。

答えは `ws://` か `wss://` の URL でなければ使わない。`https://` で問い合わせたのに暗号化されない `ws://` が返ったときも繋がない（REQ-CON-029）。問い合わせで送られてきた先に、端末の証明（ADR-024）を渡すことになるためである。

HTTP の問い合わせには `reqwest`（rustls）を使う。WebSocket と同じ TLS の実装（rustls・aws-lc-rs）なので、別の暗号の実装は入らない。

### 7. ベータは本番と別のサーバー（ステージング）を向く

`release.yml` の `prepare` ジョブがタグに `-` を含むかどうかでベータと本番を見分けている（Context 冒頭のコメント）。この判定をそのままサーバーの選択にも使う。

| リリースの種別 | タグの形 | 使う secret | 向く先 |
|--------------|---------|------------|-------|
| リリース（`vX.Y.Z`） | `-` を含まない | `JAMJAM_SERVER_URL` | 本番 |
| ベータ（`vX.Y.Z-beta.N`） | `-` を含む | `JAMJAM_SERVER_URL_BETA` | ステージング |
| リハーサル（`workflow_dispatch`、タグ無し） | — | `JAMJAM_SERVER_URL` | 本番（公開されないので実害は無いが、既定は本番のまま） |

`build.yml`（`main` / `develop` への push・PR の検証ビルド）は配布しないので対象外。既定どおり `JAMJAM_SERVER_URL` を使う。

`scripts/check-release-server-url.sh` にも同じ条件で選んだ URL を渡す。ビルド時に渡した URL とバイナリを比べる検査なので、ベータでは「バイナリがステージングの URL を持っている」ことを確認する形になる。

## Consequences

### 良い影響

- リリースビルドが本番のサーバーに問い合わせ、本番のシグナリングサーバーに接続する
- シグナリングを移してもアプリを出し直さなくてよい
- 接続画面と自己診断が同じサーバーを使う。`config.toml` の上書きも両方に効く
- 開発時の手順（`cargo tauri dev`、GUI E2E のシグナリングサーバーの起動）は変わらない

### 悪い影響・残る作業

- リリースを手元で組むときも `JAMJAM_SERVER_URL` を渡す必要がある
- 繋ぐたびに HTTP の往復が 1 回増える
- リリースビルドのまま手元のサーバーに繋ぐには `config.toml` を書く必要がある（以前は `.env.*` を置いてビルドし直していた）
- ADR-025 Decision 8 の「アプリはシグナリング URL をビルド時に解決する（`VITE_SIGNALING_SERVER`）」は本 ADR で置き換える。ハーネスが固定ポート `17890` を占有する理由（アプリが既定値に接続する）は変わらない

## 関連

- [ADR-025: GUI E2E 制御チャネル](./ADR-025-gui-e2e-control-channel.md)
- [ADR-029: 配布前に固めるアプリ設定](./ADR-029-distribution-app-settings.md)
- [architecture.md §6.4 クライアントの接続先](../architecture.md)
