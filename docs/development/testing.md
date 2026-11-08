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
| `tests/port_mapping_test.rs` | ルーターへのポート開放（UPnP）。偽のルーター（SSDP と SOAP）に対する統合テスト |
| `tests/port_prediction_test.rs` | 宛先ごとにポートを変える NAT の模型（ループバック）が、候補に複数のポートを並べること |

## ルーターのポート開放（UPnP）

ルーターにポートを開けてもらう機能（ADR-113）は、2 通りで確かめます。

- `cargo test --test port_mapping_test`: 手元のループバックに立てた偽のルーターに対して、頼みが通る・断られる・同じ番号が使用中・二重の NAT・ルーター無し・更新・閉じる、を確かめます。毎回回ります。
- `tests/upnp/run.sh`: 本物のルーター（miniupnpd）をコンテナで動かし、ルーターの先の端末から送ったデータグラムが音声のソケットに届くか、閉じると転送の規則が消えるか、UPnP の無いルーターでも STUN だけで続くか、設定オフでルーターに触れないかを確かめます。docker と、コンテナに `NET_ADMIN` を渡せる権限が要ります。CI では回しません。

## 宛先ごとにポートを変える NAT（ポートの予測）

相手の NAT が宛先ごとにポートを変えるとき、相手の送り出すポートを予測して送る機能（ADR-115）は、3 通りで確かめます。

- `cargo test --lib port_prediction` と `cargo test --lib network::connection`: ポートの並びごとの予測・送る量の上限・応答だけを受け入れること・NAT の模型（ループバック）で経路が相手の実際のポートへ移ること。毎回回ります。
- `cargo test --test port_prediction_test`: 宛先ごとにポートを変える NAT が候補に複数のポートを並べ、1 つのポートを使う NAT は並べないこと。毎回回ります。
- `tests/nat/run.sh [ケース名 ...]`: 2 つのルーター（カーネルの iptables）の内側のアプリが、STUN サーバーを介して接続できるかを、NAT の組み合わせごとに確かめます（cone・random・宛先アドレスごとに順に割り当てる sequential・ポートセットの portset）。`NAT_LAB_AUDIO_SECS=10` を付けると、経路が決まったあとに音声を送り、遅延と欠落も出します。`NAT_LAB_PEER=<バイナリ>` で別の版のアプリと比べられます。ルーターが「こちらが送るより前に届いた相手のパケット」を覚える組（`record-*`）と、両端が黙って順番に先に送り直して抜ける組（`patient-*`、1 ケース 5 分前後。ADR-119）もあります。docker と、コンテナに `NET_ADMIN` を渡せる権限が要ります。CI では回しません。

## ルームの人数

ルームは最大 16 人（相手 15 人）。1 台のアプリは相手 15 人ぶんの音声（毎秒 11250 パケット）を受けて送る。
16 人ぶんのアプリを 1 台のマシンで動かすと、相互 240 本の接続を 1 台で捌くことになるので、**理想の確かめは人数ぶんのマシン**。
いまのマシンで確かめられた範囲を、理論値と分けて表に持つ（測った日・マシン・ビルドを必ず添える）。

| 確かめ方 | 理論値 | 検証済み | 測った日・条件 |
|---|---|---|---|
| コアの結合テスト（`tests/mesh_connection_test.rs`） | 16 人 | 16 人（全員が全員の音を受ける） | 毎回 |
| 実アプリ 1 台と軽い相手 9 個（最適化ビルド） | 相手 15 人 | 相手 9 人（全パケット受信・空読み 0・往復 4.0 ms・受信スレッド約 25%） | 10/13、16 スレッド |
| 実アプリを 1 台のホストに N 個立てる（`jamjam-room-check.py`、最適化ビルド） | 16 人 | 6 人は全員が全員の音を聞いた。10 人は 1 回目は聞こえたが、続けると欠けた（ホストが飽和、load 30 超）ので品質の結論にしない | 10/13、16 スレッド |
| 実アプリを GitHub-hosted runner に分けて立てる（`room-check.yml`、最適化ビルド。ADR-117） | 16 人 | 16 人（4 台×4 人・2 台×8 人のどちらも、全員が全員の番を 93〜100 % の窓で聞いた。他人の番に聞こえたことは無い）。runner の CPU は共有なので性能の結論にはしない | 11/7、毎晩 |
| 同じ確認に Windows と macOS の runner を混ぜる（`room-check.yml`、ADR-118） | 16 人 | 10〜11 人（Linux 2 台・Windows 2 台・macOS 2〜3 台）。Linux と Windows の組は全員が聞こえた。macOS の参加者が 1 人、一部の相手と確認に失敗したまま 4 分間つながらないことが直近 4 回で毎回あり、全員の合格には届いていない | 11/8、手動 |
| GUI E2E（デバッグビルド、`tests/e2e/tests/gui.rs`） | 相手 15 人 | 相手 2 人（3 人のルーム） | 10/14、16 スレッド、ホストの load 17〜21 |
| 人数ぶんの別々のマシン | 16 人 | 未検証 | — |

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
