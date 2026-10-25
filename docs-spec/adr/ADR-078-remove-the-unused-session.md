---
sidebar_label: "ADR-078: Remove the Unused Session"
sidebar_position: 78
---

# ADR-078: 使い手のない複数人用の部品 `Session` を消す

## Status

Accepted

## Context

`network::Session` は、複数の相手へ 1 つの型で送るために作った部品である。ルームの全員と繋ぐ形（[ADR-077](./ADR-077-every-participant-is-linked.md)）は、相手ごとの `Connection` と 1 つの音声ソケット（`UdpMux`）で組み、`Session` を使わない。アプリも CLI も `Session` を作らず、作っていたのはテストだけだった。

使われない型を残すと、接続・鍵・暗号化のように `Connection` に入れた直しを `Session` にも入れるべきかを毎回考えることになる。実際、`Session` は `Connection` が持つ接続の確認・FEC・ロスの計測・RTT・再接続を持たず、残すほど `Connection` と食い違う。

## Decision

`Session`・`SessionConfig`・`src/network/session.rs` を消す。次も同時に消す。

- `NetworkError::SessionFull`・`NetworkError::PeerNotFound`（`Session` だけが作っていた）
- `Session` だけを試していたテスト。`tests/connection_test.rs` の 5 件（作る・設定・同じポートでの作り直し）と、`tests/wire_encryption_test.rs` の 3 人で暗号化を確かめる 2 件
- 要求 REQ-CON-020（ルームの上限 16 人）・REQ-SEC-001・REQ-SEC-007 の検証は、相手ごとの `Connection` を使う `tests/mesh_connection_test.rs`（16 人のルーム・宛先の違う音声を開けない）と、`tests/wire_encryption_test.rs` の 2 者間のテストが引き続き担う

[ADR-064](./ADR-064-encrypt-the-audio-link.md) と [ADR-067](./ADR-067-check-the-peer-in-the-key-exchange.md) の「実装」欄が挙げる `src/network/session.rs` は、この ADR で無くなる。両 ADR の決定は `Connection` と `SecureLink` に残っており、変わらない。

## Consequences

- 公開 API から `Session`・`SessionConfig` が無くなる。使い手は無い（ルートのパッケージ・`src-tauri`・`tests/e2e`・サーバー側のリポジトリを調べた）。
- 複数の相手へ送る形が必要になったら、`Connection` を相手ごとに持つ形（ADR-077）に載せる。
