---
sidebar_label: "ADR-090: Refuse Peers That Cannot Be Checked"
sidebar_position: 90
---

# ADR-090: 相手を確かめられない接続を無くす（署名なしの鍵交換と平文への降格の廃止）

## Status

Accepted。[ADR-064](./ADR-064-encrypt-the-audio-link.md) の「暗号化できない相手には平文で繋ぐ」と、[ADR-067](./ADR-067-check-the-peer-in-the-key-exchange.md) の「相手の鍵が渡されていない接続は、署名の無い鍵交換で暗号化して繋ぐ」を取り消す。署名の対象、鍵の導き方、echo・ボットが署名して答えること（[ADR-069](./ADR-069-sign-the-answer-of-echo-and-the-quality-bot.md)）は変えない。

## Context

ADR-064 と ADR-067 は、まだ署名を知らない古い版のアプリがルームにいても繋がるように、2 つの逃げ道を残していた。

- 相手が `link_key` を知らせないとき、署名の無い鍵交換（suite 0x01）で暗号化して繋ぐ。相手が本人かは確かめない。
- 相手が暗号化できない（鍵を送らず、平文の音声・ping を送る）とき、平文で繋ぐ。

どちらも、割り込める者には引き下げの入口になる。署名の無い鍵交換を受け付ける経路が 1 つでもあれば、割り込む者は署名つきの鍵交換を署名なしの自分の鍵に差し替えて、
両端と別々に鍵を作れる。平文を受け付ける経路が 1 つでもあれば、暗号化を黙って外される。ADR-067 は「相手の鍵が分かっているリンクは署名なしに落ちない」で守っていたが、
**鍵が分かっているかどうかを決めるのが、相手が `link_key` を知らせたかどうか**なので、`link_key` を取り除く側に、引き下げの入口が残っていた。

古い版のアプリは検証用の利用者に限られていて（試験版 beta.51 より前）、切り捨てて構わないと uchida が判断した（10/26）。

## Decision

1. **署名の無い鍵交換を作らない・受け付けない。** suite は署名つきの 0x02 だけ。署名の無い鍵交換は、アプリの側でも、echo・ボットの側でも捨てる。
2. **平文は受け付けない。** 鍵が合う前に受け付けるのは keep-alive だけ。鍵が合う前の平文の音声・ping・遅延の情報は捨て、鍵が合ったあとに平文が届いても受け付けない。接続の状態は `Negotiating` と `Encrypted` の 2 つだけ（`Unencrypted` を無くす）。
3. **相手の鍵が無い・読めない接続は繋がない。** `Connection::verify_peer` を相手の `link_key`（必須）で呼ぶまで `connect` は `NetworkError::PeerKeyMissing` で終わり、パケットを 1 つも出さない。
   `ConnectionStats::peer_checked` と `Connection::checks_peer` の「確かめない接続」の区別は、全ての接続が確かめるので意味を失い、`peer_checked` を無くす。
4. **画面は「この相手とは繋げません（相手のアプリが古い版です）」と出す。** リンクは `failed` で、`connection_error` を `peer_app_too_old` にし、再接続を促さない（相手が更新するまで再試行しても繋がらない）。
5. echo と音質チェックのボットは `link_key` を知らせて署名して答える（ADR-069）。署名の無い鍵交換には答えない。

## Consequences

- **古い版のアプリとは、音声が繋がらなくなる。** 試験版 beta.51 より前の版を使っている人は、更新するまで相手と繋がらない（画面に理由が出る）。
- 引き下げの入口が無くなる。割り込む者が `link_key` を取り除いても、署名つきの鍵交換しか受け付けないので、暗号化された通信も、確かめられた相手も、落とされない。
- 遅延と音質は変わらない。暗号化・復号の計算は ADR-064 と同じで、鍵が合うまで音声を待つ時間も同じ（古い相手を見分けるための待ちが無くなる分、むしろ短い）。
- 新しいアプリどうしの接続の手順（署名つきの鍵交換 1 往復）は変わらない。

## References

- 要求: REQ-SEC-004・006・007・008・009（[requirements.md](../requirements.md)）
- [ADR-064](./ADR-064-encrypt-the-audio-link.md)・[ADR-067](./ADR-067-check-the-peer-in-the-key-exchange.md)・[ADR-069](./ADR-069-sign-the-answer-of-echo-and-the-quality-bot.md)
