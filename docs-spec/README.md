---
sidebar_label: Overview
sidebar_position: 1
---

<!-- このドキュメントは実装の正です。変更時は実装も同期すること -->

# jamjam Specification Documents

本ディレクトリは仕様書を格納する。
すべてのドキュメントは実装の唯一の正とする。

---

## ドキュメント構成

| ファイル | 説明 |
|---------|------|
| [architecture.md](./architecture.md) | 技術構成（最重要） |
| [requirements.md](./requirements.md) | 要求仕様（V字左辺の最上位・REQ-ID 定義） |
| [traceability.md](./traceability.md) | 要求と検証の対応表（自動生成） |
| [ui-ux-guideline.md](./ui-ux-guideline.md) | UI/UXガイドライン |
| [adr/](./adr/ADR-001-language-rust.md) | 設計判断記録（ADR） |
| [api/](./api/audio_engine.md) | API境界定義 |
| [behavior/](./behavior/connection.feature) | 振る舞い定義（BDD/Gherkin） |
| [ui/](./ui/README.md) | UI仕様（画面・コンポーネント・デザイントークン） |

---

## ADR（設計判断記録）

| ADR | 決定内容 |
|-----|---------|
| [ADR-001](./adr/ADR-001-language-rust.md) | Rust採用 |
| [ADR-002](./adr/ADR-002-network-protocol.md) | カスタムUDPプロトコル採用 |
| [ADR-003](./adr/ADR-003-audio-codec.md) | 音声コーデック選択（複数対応） |
| [ADR-004](./adr/ADR-004-gui-framework.md) | GUIフレームワーク選択（Tauri / Flutter） |
| [ADR-005](./adr/ADR-005-no-audio-processing.md) | 音声処理を行わない方針 |
| [ADR-006](./adr/ADR-006-fec-strategy.md) | FEC（前方誤り訂正）採用 |
| [ADR-007](./adr/ADR-007-i18n-library.md) | i18nライブラリ選定 |
| [ADR-008](./adr/ADR-008-zero-latency-mode.md) | ゼロレイテンシーモード |
| [ADR-009](./adr/ADR-009-tauri-build-commands.md) | Tauriビルドコマンド戦略 |
| [ADR-011](./adr/ADR-011-core-library-architecture.md) | コアライブラリアーキテクチャ |
| [ADR-012](./adr/ADR-012-code-signing-strategy.md) | コード署名戦略 |
| [ADR-013](./adr/ADR-013-sample-rate-strategy.md) | サンプリングレート戦略 |
| [ADR-014](./adr/ADR-014-claude-code-config-structure.md) | Claude Code 設定構成の再編 |
| [ADR-016](./adr/ADR-016-remove-host-privilege-concept.md) | ルーム作成者特権（ホスト概念）の廃止 |
| [ADR-018](./adr/ADR-018-iterative-v-model-traceability.md) | 反復V字モデルと要求トレーサビリティの採用 |
| [ADR-019](./adr/ADR-019-preset-latency-budget.md) | プリセット遅延バジェットの確定（ADR-008の概算表を上書き） |
| [ADR-020](./adr/ADR-020-jitter-buffer-wiring.md) | ジッタバッファの受信経路への配線と実効遅延の修正 |
| [ADR-021](./adr/ADR-021-preset-codec-and-fec.md) | プリセットのコーデックとFEC設定（Opusのフレームサイズ非互換を記録） |
| [ADR-022](./adr/ADR-022-reconnection-and-narrowband-scope.md) | 再接続の設計と狭帯域回線・カスタムプリセットの対象外化 |
| [ADR-023](./adr/ADR-023-drop-phase-as-identifier.md) | 「Phase」を識別子として使わない（Plans.md は要求ID主キー） |
| [ADR-024](./adr/ADR-024-device-identity-instead-of-accounts.md) | 端末アイデンティティによる識別（メールOTPアカウントを上書き） |
| [ADR-025](./adr/ADR-025-gui-e2e-control-channel.md) | GUI E2E 制御チャネルとページオブジェクトモデルの導入 |
| [ADR-026](./adr/ADR-026-gui-audio-path.md) | GUI 同士の音声経路（アドレス交換・受信ループのペーシング・ジッタバッファの損失判定） |
| [ADR-027](./adr/ADR-027-cli-scope.md) | CLI の位置づけ（デバッグ用途）と GUI との機能差・設定ファイルの共有 |
| [ADR-028](./adr/ADR-028-single-stage-playout.md) | 受信音声を単段バッファにし、出力コールバックが直接引く |
| [ADR-029](./adr/ADR-029-distribution-app-settings.md) | 配布前に固めるアプリ設定（識別子・CSP・マイク使用の説明） |
| [ADR-030](./adr/ADR-030-signaling-url-by-build-profile.md) | シグナリングサーバーの接続先をビルドの種別で決め、1 か所に置く |
| [ADR-033](./adr/ADR-033-local-monitoring.md) | ローカルモニタリングを、出力コールバックへ入力を直接混ぜて実現する |
| [ADR-035](./adr/ADR-035-stun-through-the-audio-socket.md) | 公開アドレスは音声ソケット自身から STUN に問い合わせて公開する |
| [ADR-036](./adr/ADR-036-diagnostic-log-file.md) | 公開ビルドでも診断ログファイル `jamjam.log` を書く（画面側の出力と失敗したコマンドを含む） |
| [ADR-037](./adr/ADR-037-usage-reporting-opt-in.md) | 利用状況の送信は、利用者が設定でオンにしたときだけ。送るもの・外す 5 項目・止め方 |

---

## API仕様

| API | 説明 |
|-----|------|
| [audio_engine.md](./api/audio_engine.md) | オーディオエンジンAPI |
| [network.md](./api/network.md) | ネットワークAPI |
| [signaling.md](./api/signaling.md) | シグナリングAPI |
| [i18n.md](./api/i18n.md) | 国際化API |
| [device-identity.md](./api/device-identity.md) | 端末アイデンティティ（`X-Device-*` ハンドシェイクヘッダー） |
| [e2e-control.md](./api/e2e-control.md) | GUI E2E 制御チャネル（テスト専用・既定で無効） |
| [telemetry.md](./api/telemetry.md) | 利用状況の送信（既定オフ。送る項目・送り方・止め方） |

---

## BDD仕様

| Feature | 説明 |
|---------|------|
| [connection.feature](./behavior/connection.feature) | セッション接続 |
| [audio-quality.feature](./behavior/audio-quality.feature) | 音声品質 |
| [latency.feature](./behavior/latency.feature) | 遅延管理 |
| [i18n.feature](./behavior/i18n.feature) | 国際化 |

各 Scenario は `@REQ-<領域>-<番号>` と `@must` / `@should` のタグを持つ。ID 体系は [requirements.md](./requirements.md)、検証との対応は [traceability.md](./traceability.md) を参照。

---

## 更新ルール

1. **実装が仕様に影響を与える変更を行った場合、同一コミットで仕様書も更新する**
2. ADR は追加のみ（既存 ADR の変更は原則禁止、変更が必要な場合は新規 ADR を作成）
3. API 仕様は実装と常に同期を維持する
4. BDD 仕様はテストコードと対応させる（`@REQ-*` タグとテストの `Verifies:` 注釈で機械検証する）
5. 要求・テストを増減させたら `docs-spec/traceability.md` を再生成する（`JAMJAM_UPDATE_TRACEABILITY=1 cargo test --test traceability_test`）

---

## 開発者向けドキュメント

開発者向けの解説資料（ガイド、チュートリアル等）は [Docs](/docs/intro) を参照。

> docs-site/ の内容は仕様ではない。実装の正は常に本ディレクトリ（docs-spec/）である。
