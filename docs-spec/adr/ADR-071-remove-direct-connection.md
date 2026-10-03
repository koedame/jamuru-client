---
sidebar_label: "ADR-071: Remove the Direct Connection"
sidebar_position: 71
---

# ADR-071: サーバーを使わない直接接続（CLI の `host` / `join`）を無くし、往復の測定はルームで行う

## Status

Accepted

## Context

CLI には、シグナリングのサーバーを使わずにアドレスを指定して繋ぐ `host`（待ち受け）と `join`（接続）があった（[ADR-027](./ADR-027-cli-scope.md)）。
GUI にはこの機能が無く、利用者はルームで繋ぐ。

直接接続には、サーバーが相手の鍵を渡す道が無い。ルームの相手とは [ADR-067](./ADR-067-check-the-peer-in-the-key-exchange.md) の署名で
鍵交換を確かめるが、直接接続はこれができず、暗号化はするが相手が本人かは確かめない接続（`ConnectionStats::peer_checked` が偽）だった。
確かめる方式（共有の秘密から鍵交換を認証する、鍵の指紋を両端で見比べる）を足すか、確かめないと割り切るかを決める必要があった。

どちらの方式も、CLI の直接接続にしか使わない認証の処理を新しく足す。直接接続が頼っていたのは、次の 2 つだけだった。

- 往復遅延の測定（`--duration` `--report-json` `--echo-delay-ms`。[ADR-039](./ADR-039-cli-round-trip-measurement.md)）
- CLI のテストと夜間の E2E が、サーバーを立てずに 2 つのプロセスを繋ぐ入り口

測定の対象は音声の経路（取り込み・送信・暗号化・受信・ジッタバッファ・再生）で、繋ぎ方には依存しない。繋がったあとの経路は、
ルームでも直接接続でも同じ `Connection` と `AudioSession` である。

## Decision

1. **直接接続を無くす。** `host` / `join` のコマンドと、それだけが使っていた `Connection::accept` を削除する。相手を確かめない経路は、古い版のアプリとの接続だけになる。
2. **往復の測定は `create-room` / `join-room` で行う。** `--duration` `--report-json` `--echo-delay-ms` を両方に付ける。`--duration` は相手につながってからの秒数で、
   レポートの形は変えない（`peer` は繋がった相手のアドレス）。測る相手は、音を保持して返す echo（[ADR-069](./ADR-069-sign-the-answer-of-echo-and-the-quality-bot.md)）で、
   echo はルームにいるので、ルームで繋ぐだけで測れる。
3. **テストは、サーバーの代役を立ててルームで繋ぐ。** `tests/common/fake_signaling.rs` が、ルームの作成・参加・アドレスの公開・チャット・退出を実装し、
   公開されたアドレスを `127.0.0.1` に直して知らせる。CLI のテストと E2E の 2 ノードのテストが、これを使う。
4. **GUI には測定を足さない。** 測定は `--input-bursts`（合成した入力）と `--output-file`（出力の代わり）で、デバイスの無い環境でも回る CLI の機能である。GUI の取り込み経路を測るものではない（ADR-039）。

## Consequences

- **サーバーを使わずに繋ぐ道が、アプリにも CLI にも無くなる。** 相手の確認を例外なく行える（古い版のアプリとの接続を除く）。
  プライバシーのページから、「サーバーを使わない直接接続は確かめない」の記述を消した。
- **測定の結果は変わらない。** 同じ echo の相手（保持 200 ms）に、直接接続とルームの両方で 3 回ずつ測った。

  | 経路 | 往復の中央値（ms） |
  |------|-------------------|
  | 直接接続（変更前） | 15.99 / 13.31 / 13.39 |
  | ルーム（変更後） | 13.33 / 16.06 / 13.31 |

  ばらつきは同じ範囲で、約 13.3 ms と約 16 ms のどちらかになる（フレームの位相による 1 フレームぶんの差）。
  ルームには入室のときの署名の確認と、繋ぐ候補の選び方（近い経路から）が足されるが、どちらも繋ぐときだけで、繋がったあとのパケットごとの処理は変わらない。
- **測定にはサーバーが要る。** 本物の echo を測るなら本物のサーバーに繋ぐ。テストはサーバーの代役を使うので、サーバーなしで回る。
- 標準入力が閉じている CLI（`--duration` で終わらせるスクリプト、テスト）は、閉じたことを検出して標準入力を読むのをやめる。
  これまでルームのセッションは、閉じた標準入力を読み続けて回り続けた。

### 検証

| 要求 | 検証 |
|------|------|
| REQ-CLI-005 | `tests/cli_test.rs::two_clis_in_a_room_hear_each_other` |
| REQ-CLI-006 | `tests/cli_test.rs`（ルームに 1 人でいる CLI のモニタリング） |
| REQ-CLI-007 | `tests/cli_test.rs::a_cli_joined_to_an_echo_reports_the_round_trip_of_its_bursts` |

夜間の E2E の 2 ノードのテスト（`tests/e2e`）も、同じ代役のサーバーで `create-room` / `join-room` の 2 プロセスを立てる。

## 関連

- [ADR-027: CLI の位置づけ](./ADR-027-cli-scope.md)
- [ADR-039: 往復遅延の測定](./ADR-039-cli-round-trip-measurement.md)
- [ADR-067: 鍵交換で相手を確かめる](./ADR-067-check-the-peer-in-the-key-exchange.md)
- [ADR-069: echo と音質チェックのボット](./ADR-069-sign-the-answer-of-echo-and-the-quality-bot.md)
