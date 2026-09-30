---
sidebar_label: "ADR-025: GUI E2E Control Channel"
sidebar_position: 25
---

<!-- このドキュメントは実装の正です。変更時は実装も同期すること -->

# ADR-025: GUI E2E 制御チャネルとページオブジェクトモデルの導入

## Status

Accepted

## Context

**V字モデルの右辺に、GUI アプリを動かす層が存在しなかった。**

[ADR-018](./ADR-018-iterative-v-model-traceability.md) の検証層の対応表は、実装層に単体テスト、
API 層に結合テスト、アーキテクチャ層にシステム E2E（`tests/e2e/` = 音声・ネットワーク）を割り当てて
いる。しかし `src-tauri/` と `ui/` を統合した「アプリが起動して画面が出て、押すと反応する」状態を
検証する層がなかった。

- Storybook（`ui/`、225 ストーリー）は Pure コンポーネントのみを描画する。`ui-component-rules.md`
  が Adapter に Tauri 依存を隔離しているため、**Storybook は `invoke()` の配線を一切通らない**。
- UI 単体テスト（`ui/src/**/*.test.tsx`）も同じ範囲に留まる。
- `tests/e2e/` は音声パスとネットワークを対象とし、GUI を起動しない。

その結果、検証手段が「人間が `cargo tauri dev` を起動して目視する」しかなく、Plans.md の
「手動確認待ち」に項目が溜まり続けていた（テストルーム機能、招待リンク、ADR-024 の端末識別子）。
これらは数か月単位で未チェックのまま残っていた。

## Decision

### 1. アプリに E2E 制御チャネルを追加する

`src-tauri` に cargo feature `e2e-control` を追加し、有効時のみループバック HTTP で
レンダリング済み DOM を公開する（`src-tauri/src/e2e_control.rs`）。

| Method | Path | 用途 |
|--------|------|------|
| GET | `/e2e/health` | 起動待ち合わせ |
| GET | `/e2e/windows` | 開いているウィンドウのラベル一覧 |
| POST | `/e2e/dom` | `document.documentElement.outerHTML` |
| POST | `/e2e/query` | セレクタ指定の構造化クエリ |
| POST | `/e2e/click` | クリック |
| POST | `/e2e/input` | 入力（`<select>` の選択も含む） |

詳細は [e2e-control.md](../api/e2e-control.md)。

### 2. 二重に無効化する

| 条件 | 動作 |
|------|------|
| feature 無効（既定） | コードがバイナリに存在しない |
| feature 有効 + `JAMJAM_E2E_CONTROL_PORT` 未設定・不正 | ポートを開かない |
| feature 有効 + 環境変数あり | `127.0.0.1:<port>` で待ち受け |

`#[cfg(debug_assertions)]` ではなく cargo feature にした理由は、`debug_assertions` だと開発ビルドで
常にポートが開いてしまうためである。feature ならリリース成果物にコードが物理的に存在しない。

ポートを探索させず**ハーネスが指定する**方式にしたことで、ポート番号を書き出すファイルが不要になり、
かつ feature が誤って既定に入っても環境変数なしでは不活性になる。

`tests/release_build_guard_test.rs` が `src-tauri/Cargo.toml` の `[features] default` を読み、
`e2e-control` が含まれないことを検証する。feature を付けずに `cargo test` で常時走る。
ビルド成果物のシンボル走査ではなく設定の検証に留めたのは、CI でリリースビルドを走らせるコストを
避けるためである。

### 3. DOM を先に、スクリーンショットは後に

第1段では DOM の取得と操作のみを実装し、スクリーンショットは実装しない。DOM は差分が取れて決定的で
あり、そのままアサーションに使える。スクリーンショットはレイアウト崩れの検出には要るが、判定に人間か
VRT ツールを別途必要とする。今回自動化した検証項目はすべて DOM で判定できた。

### 4. `eval_with_callback` で JS の結果を受け取る

Tauri 2.11.5 の `WebviewWindow::eval_with_callback` が JS の評価結果を JSON 文字列でコールバックに
返す。独自の JS ブリッジは不要である。`tokio::sync::oneshot` で受け、5 秒でタイムアウトさせる
（無応答の webview がハーネスを固めないため）。

Tauri は Windows で `eval_with_callback` の例外が無視されると文書化している。評価式を
`try { ... } catch (e) { return { __e2e_error: String(e) } }` で包み、例外を戻り値として返すことで、
スクリプトの誤りが「区別のつかないタイムアウト」になることを避ける。

セレクタと入力値は `serde_json::to_string` で JS 文字列リテラルへエンコードする。これがないと
セレクタに含まれる引用符でリテラルを抜け出し、任意のスクリプトを実行できてしまう。

### 5. 入力は React が観測できる形で行う

`el.value = x` だけでは不十分である。React は DOM ノードに前回値を記録しており、直接代入を
「変化なし」と見なすため `onChange` が発火せず、コンポーネントの状態が更新されない。
プロトタイプのネイティブ setter を経由し、`input` イベントをバブリング付きで dispatch する。

### 6. `data-testid` で要素を指す

クラス名や表示文字列に依存すると、スタイル変更や i18n で壊れる（実際にアプリは英語ロケールで
起動する）。Pure コンポーネントのルートと主要な操作要素に `data-testid` を付ける。
規約は `.claude/rules/ui-component-rules.md`。

既に安定した意味を持つ属性がある場合はそれを使う。設定タブは `id="tab-<id>"` と
`aria-selected` を持つため、`data-testid` を追加していない。

### 7. テストとアプリの間にページオブジェクトモデルを挟む

`tests/e2e/src/pom/` に画面ごとのページオブジェクトを置き、シナリオはセレクタ・ウィンドウ
ラベル・HTTP を一切見ない。`Driver` は `pub(crate)` に留め、シナリオから直接セレクタを投げられない
ようにする。これをしないとページオブジェクトが形骸化し、生セレクタがテストに散る。

```rust
let screen = app.connection_screen();
screen.invite_code_input().type_text("ABC234XYZ")?;
assert!(screen.join_button().is_enabled()?);
```

### 8. 複数ピアはアプリを複数起動して作る

アプリはシングルインスタンス化していない（`tauri-plugin-single-instance` を使っていない）ため、
同時に何個でも起動できる。各インスタンスは独自の制御ポートと `$HOME` を持つので、干渉しない。

2 台のアプリを同じルームに入れるシナリオは、シグナリングサーバーを立てて回すテストであり、
このリポジトリの外で回す。

アプリはシグナリング URL をビルド時に解決する（`VITE_SIGNALING_SERVER`、既定
`ws://localhost:17890`）ため、サーバーを立てる側はエフェメラルポートを使えず、アプリがダイヤルする固定
ポートを占有する必要がある（解決の仕方は ADR-030 で置き換えた。開発ビルドの既定は
`http://localhost:17890` のサーバーで、固定ポートを占有する理由は変わらない）。GUI シナリオは直列実行なのでテスト間の競合にはならない。

### 9. テストは `$HOME` を差し替えて起動する

`App::launch()` は一時ディレクトリを `$HOME` として渡す。`directories` クレートがアプリデータ
ディレクトリを `$HOME` から導出するため、これだけで開発者の実 `config.toml` と
`device_identity.json` を汚さずに済む。**製品コードにテスト専用の上書き機構を入れる必要がない。**

## 脅威モデル・セキュリティ対策

| リスク | 対策 |
|--------|------|
| 制御チャネルがリリースビルドに混入する | cargo feature が既定で無効。`release_build_guard_test.rs` が常時検証 |
| 混入した場合に外部から到達される | ループバックのみにバインド。加えて環境変数未設定では待ち受けを開始しない |
| セレクタ経由のスクリプト注入 | セレクタ・入力値を `serde_json` で JS 文字列リテラルへエンコード |
| 無応答の webview でハーネスが停止する | eval に 5 秒のタイムアウト、超過時は 504 |
| テストが開発者の設定・端末識別子を破壊する | 起動時に `$HOME` を一時ディレクトリへ差し替え |

## Consequences

### メリット

- 手動確認でしか担保できなかった項目が自動テストになった（REQ-GUI-001〜010）。
- Storybook では到達不能な統合の欠陥を検出できる。実際に導入直後、起動時の自動接続失敗
  （「Connection Refused」）が DOM に現れ、Adapter の `invoke()` 配線が動いていることを確認できた。
- ページオブジェクトを挟んだことで、UI の作り替えでテスト本体を書き換えずに済む。
- 2 ピアを立てられるため、参加者一覧・ミキサーのチャンネル追加・ミュート・チャットの双方向
  送受信まで自動化できた（REQ-GUI-008〜010）。導入前は手動確認しか手段がなかった領域である。

### デメリット

- ビルド成果物の鮮度に依存する。`cargo build`（feature なし）が同じパスのバイナリを上書きするため、
  古いバイナリで起動して「ポートが開かない」ように見える事故が起きる。`App::launch()` は
  この失敗を「feature 付きでビルドしたか」を示すメッセージで報告する。
- GUI シナリオは実アプリを起動するため、`--test-threads=1` での直列実行が要る（オーディオデバイスの
  競合を避けるため）。
- macOS では `tauri-driver`（公式 WebDriver）が使えないため、この自作チャネルが唯一の手段になる。
  他プラットフォームでも同じチャネルを使うことで、経路をひとつに保つ。

### 制約

- `e2e-control` を `default` に入れてはならない。
- 制御チャネルはループバック以外にバインドしてはならない。表示中のルームコード・チャット・
  参加者名を読めるためである。
- 実オーディオ信号を要する状態（入力レベルメーターの反応、音量・パン操作の音への反映）を
  `must` 要求として立てない。仮想オーディオデバイス（`tests/e2e/src/virtual_audio.rs`）と
  音声品質測定が必要で、それはループバック E2E の領域である。

## 関連ドキュメント

- [e2e-control.md](../api/e2e-control.md) — エンドポイントの詳細仕様
- [ADR-018: 反復V字モデルとトレーサビリティ](./ADR-018-iterative-v-model-traceability.md) — 本 ADR が埋める検証層の定義元
- [ADR-024: 端末アイデンティティによる識別](./ADR-024-device-identity-instead-of-accounts.md) — REQ-GUI-002 の由来
- [ADR-009: Tauri ビルドコマンド](./ADR-009-tauri-build-commands.md) — ビルド実行位置の制約
- [requirements.md](../requirements.md) — `REQ-GUI-001` 〜 `REQ-GUI-011`
