> [!NOTE]
> このドキュメントは開発者向けの解説資料です。
> 正確な仕様・制約・判断は [docs-spec/](../../docs-spec) を参照してください。

# テスト

jamuruのテストを実行する方法を説明します。

## テストの実行

### 全テスト実行

```bash
cargo test --all-targets
```

### 特定のテストを実行

```bash
# テスト名で絞り込み
cargo test test_name

# 特定のモジュールのテスト
cargo test audio::
cargo test network::
```

### テストの詳細出力

```bash
cargo test -- --nocapture
```

## テスト構成

テストは [docs-spec/behavior/](../../docs-spec/behavior) の BDD 仕様に基づいています。

| テストファイル | 対応仕様 |
|--------------|---------|
| `tests/connection_test.rs` | `docs-spec/behavior/connection.feature` |
| `tests/audio_quality_test.rs` | `docs-spec/behavior/audio-quality.feature` |
| `tests/latency_test.rs` | `docs-spec/behavior/latency.feature` |
| `tests/i18n_test.rs` | `docs-spec/behavior/i18n.feature` |

## ルームの人数

ルームは最大 10 人（相手 9 人）。1 台のアプリは相手 9 人ぶんの音声（毎秒 6750 パケット）を受けて送る。
10 人ぶんのアプリを 1 台のマシンで動かすと、相互 100 本の接続を 1 台で捌くことになるので、**理想の確かめは人数ぶんのマシン**。
いまのマシンで確かめられた範囲を、理論値と分けて表に持つ（測った日・マシン・ビルドを必ず添える）。

| 確かめ方 | 理論値 | 検証済み | 測った日・条件 |
|---|---|---|---|
| コアの結合テスト（`tests/mesh_connection_test.rs`） | 10 人 | 10 人（全員が全員の音を受ける） | 毎回 |
| 実アプリ 1 台と軽い相手 9 個（最適化ビルド） | 相手 9 人 | 相手 9 人（全パケット受信・空読み 0・往復 4.0 ms・受信スレッド約 25%） | 10/13、16 スレッド |
| 実アプリを 1 台のホストに N 個立てる（`jamjam-room-check.py`、最適化ビルド） | 10 人 | 6 人は全員が全員の音を聞いた。10 人は 1 回目は聞こえたが、続けると欠けた（ホストが飽和、load 30 超）ので品質の結論にしない | 10/13、16 スレッド |
| GUI E2E（デバッグビルド、`tests/e2e/tests/gui.rs`） | 相手 9 人 | 相手 2 人（3 人のルーム） | 10/14、16 スレッド、ホストの load 17〜21 |
| 人数ぶんの別々のマシン | 10 人 | 未検証 | — |

デバッグビルドは、相手 2 人で空読み 26・往復 5.3 ms だが、相手 3 人から空読みが数千に増え往復が 16.0 ms に張り付く（暗号と PCM の変換が間に合わない）。
同じ負荷でも最適化ビルドは相手 9 人を捌くので、必要な性能はビルドで桁が違う。測り直すとき:

```bash
JAMJAM_CAPACITY_PEERS=2,3,4,5,6,7,8,9 cargo test --features gui --test gui -- --ignored --nocapture --test-threads=1 how_many_peers_the_app_takes
```

人数ごとに、相手の数・1 本あたりの受信と送られた数・空読み・補間・往復の遅延・アプリの CPU・ホストの load が 1 行ずつ出る。結果の表は
上の表を更新する（ホストが混んでいる間の値は、load を添えて上限の目安にとどめる）。

## コード品質チェック

### フォーマットチェック

```bash
cargo fmt --check
```

### Lint (Clippy)

```bash
cargo clippy --all-targets -- -D warnings
```

### 全チェック（CI相当）

```bash
cargo fmt --check && \
cargo clippy --all-targets -- -D warnings && \
cargo check --all-targets && \
cargo test --all-targets
```

## 関連情報

- [ビルド](building.md) - ビルド方法
- [CI/CD](ci.md) - 継続的インテグレーション
