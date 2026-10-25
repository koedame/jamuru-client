<!-- このドキュメントは実装の正です。変更時は実装も同期すること -->

# jamuru Specification Documents

本ディレクトリは仕様書を格納する。
アプリの旧名は jamjam。`adr/` の決定記録は当時の名前（jamjam）のまま残している。
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
| [ADR-036](./adr/ADR-036-diagnostic-log-file.md) | 公開ビルドでも診断ログファイル `jamuru.log` を書く（画面側の出力と失敗したコマンドを含む） |
| [ADR-037](./adr/ADR-037-usage-reporting-opt-in.md) | 利用状況の送信は、利用者が設定でオンにしたときだけ。送るもの・外す 5 項目・止め方 |
| [ADR-038](./adr/ADR-038-transmit-channels-select-capture.md) | 送信チャンネル設定は取り込むチャンネル数を決める。線路は常にステレオ |
| [ADR-039](./adr/ADR-039-cli-round-trip-measurement.md) | CLI が区切りのある信号で音声経路の往復遅延を測り、JSON に残す |
| [ADR-040](./adr/ADR-040-input-output-channel-selection.md) | 入力・出力チャンネルの設定は、デバイスを開くチャンネル数と、取り出す・置く番号を決める |
| [ADR-041](./adr/ADR-041-self-update.md) | 新しい正式版は、利用者の操作なしにアプリ自身が入れる |
| [ADR-042](./adr/ADR-042-usage-reporting-includes-investigation-data.md) | 利用状況の送信に、不具合の調査に役立つ項目（自分の IP アドレス・デバイス ID・接続先）を含める |
| [ADR-043](./adr/ADR-043-remote-operation-rpc.md) | アプリの操作は名前付きのコマンド（RPC）。E2E は全部を呼ぶ（手伝いの部分は ADR-044 が置き換えた） |
| [ADR-044](./adr/ADR-044-portals-and-permissions.md) | アプリの操作は「口」ごとに呼べる範囲を 1 つの表で決める。遠隔の口は WebSocket の中継に載せ、権限はデバッグモード（ベータ版だけ）と手伝いの 2 段 |
| [ADR-045](./adr/ADR-045-beta-self-update.md) | ベータ版も自動更新する。ベータ版は `X.Y.Z-N` の版としてビルドし、更新情報は固定タグの Release `beta-channel` から読む（ADR-041 の 4 節を置き換えた） |
| [ADR-046](./adr/ADR-046-audio-gaps-are-measured-not-guessed.md) | 受信バッファの自動調整は空読みも欠けとして数える。帯域の判定は相手が送ったパケットの欠けの割合で行う。音の欠けの原因は飛行記録（`debug.audio_timing`）で測る |
| [ADR-047](./adr/ADR-047-git-flow-branching.md) | 開発は develop で進め、main には出した正式版だけを置く。ベータ版はタグを打ったときだけ公開し、タグは対になるサーバー側のリポジトリと同じ名前を同じときに打つ |
| [ADR-049](./adr/ADR-049-channels-fit-when-opened-and-pan-follows-the-sender.md) | デバイスへのチャンネルの当てはめは開くときだけ（保存は書き換えない）。同じ番号の組は 1 チャンネル。受け手のパンは届いた音がステレオかモノラルかで変わる |
| [ADR-050](./adr/ADR-050-device-calls-are-bounded.md) | 固まったオーディオドライバへの呼び出しには上限を付け、待ちが設定の読み書き・別のデバイスへの切り替え・ネットワークを止めないようにする。開けなかったデバイスは画面に出す |
| [ADR-051](./adr/ADR-051-route-follows-the-peers-answers.md) | 音声の送り先は、こちらから送れた候補の中から選ぶ。相手が ping に応答しないまま別の候補から声が届いたら、送り先をそちらに移す |
| [ADR-054](./adr/ADR-054-route-is-the-nearest-that-answers.md) | 送り先は、確認の keep-alive に返事を求めて 5 回ずつ測った往復時間の中央値が最も短い候補にする。返事を返さない相手とは、種類の順位では選ばない（1.0.0 までは互換性を保たない）。`JAMJAM_ROUTE` で経路を指定できる |
| [ADR-068](./adr/ADR-068-follow-the-peer-after-rejoin.md) | 入り直しのあと、音声は名簿の旧エントリではなく、いま聞こえる相手へ張り直す。移すとき自分のアドレスは変えない |
| [ADR-069](./adr/ADR-069-sign-the-answer-of-echo-and-the-quality-bot.md) | echo と音質チェックのボットは、受け取ったアプリの一時鍵あてに署名して答える。アプリは相手の `link_key` の署名なら、宛先が自分の鍵でも一時鍵でも受け付ける |
| [ADR-070](./adr/ADR-070-split-audio-frames-that-do-not-fit-a-packet.md) | 暗号化後に 1460 バイトを超える音声フレーム（256 フレームのステレオ PCM）は、フレーム境界で等分した複数のパケットで送る。IP の断片化に載せない |
| [ADR-071](./adr/ADR-071-remove-direct-connection.md) | サーバーを使わない直接接続（CLI の `host` / `join`）を無くし、往復の測定は `create-room` / `join-room` で行う。相手を確かめない経路が無くなる |
| [ADR-077](./adr/ADR-077-every-participant-is-linked.md) | ルームの参加者は全員と音声を繋ぐ（最大 16 人のフルメッシュ）。音声ソケットは参加者ごとに 1 つで、相手ごとの接続が共有する。相手との音声が途切れたら、その相手だけを張り直し、誰の音も届かないときだけ入り直す |
| [ADR-078](./adr/ADR-078-remove-the-unused-session.md) | 使い手のない複数人用の部品 `Session` を消す。相手ごとの `Connection` で足りる |
| [ADR-079](./adr/ADR-079-follow-the-peer-to-an-address-it-was-not-offered-from.md) | 暗号化したリンクでは、公開されていないアドレスから届いた（復号と重複の検査を通った）パケットの送り元へ、返事が無いときに送り先を移す |
| [ADR-080](./adr/ADR-080-settings-tabs-by-what-the-user-is-doing.md) | 設定のタブは「利用者がしたいこと」で分ける（一般・オーディオ・プライバシー・診断とサポート・アプリについて）。項目は減らさず置き場所だけを変える |
| [ADR-081](./adr/ADR-081-native-menu-bar.md) | メニューバーは OS ごとの作法に合わせて作り、状態に応じて項目を有効・無効にする。ヘルプを充実させ、ショートカットはまだ付けない |
| [ADR-082](./adr/ADR-082-check-and-install-updates-by-hand.md) | 更新の確認と入れ替えを、設定の「アプリについて」とメニューの「アップデートを確認…」から利用者が頼める。セッション中は断り、頼めるのはアプリ自身の画面だけ |
| [ADR-083](./adr/ADR-083-audio-session-runs-from-entering-the-room.md) | 音声のセッションはルームに入った時点で始め、相手がいなくても動かし続ける（1 人でもマイクのメーターが動く）。ADR-077 §4 を置き換える |
| [ADR-084](./adr/ADR-084-frame-size-and-sample-rate-apply-by-restarting-the-audio.md) | バッファサイズとサンプルレートは、部屋にいるときは音声のセッションをやり直して反映する。ADR-043 の「次の接続から効く」を置き換える |
| [ADR-085](./adr/ADR-085-one-copy-of-the-app-runs.md) | アプリは 1 人につき 1 つだけ動かし、2 つ目の起動は 1 つ目に渡して終わる |
| [ADR-086](./adr/ADR-086-linux-realtime-through-rtkit.md) | Linux では音声スレッドの実時間優先度を、取れなければ rtkit に頼んで取る |
| [ADR-087](./adr/ADR-087-windows-audio-threads-join-mmcss.md) | Windows では音声スレッドを MMCSS に登録して実時間の範囲を取る |
| [ADR-088](./adr/ADR-088-wasapi-exclusive-and-low-latency-modes.md) | Windows の WASAPI のデバイスを、排他モードと低遅延の共有モードでも開けるようにし、設定で選ばせる |
| [ADR-089](./adr/ADR-089-update-before-opening-and-keep-the-previous-version.md) | 起動したら本体を開く前に更新し、入れる前の版を写して残す。入れた版が起動しなければ見届け役が前の版に戻す。リリースは各 OS で更新の訓練が通るまで公開しない |

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

開発者向けの解説資料（ガイド、チュートリアル等）は [docs/](../docs/README.md) を参照。

> docs/ の内容は仕様ではない。実装の正は常に本ディレクトリ（docs-spec/）である。
