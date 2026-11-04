# ADR-110: 音声のパケットに優先の印を付ける

## Status

Accepted。

## Context

IP のパケットの見出しには、優先度を書く 6 ビットの欄がある（DSCP。IPv4 では ToS、IPv6 では Traffic Class の上位 6 ビット）。送る側がソケットに値を設定すると、そのソケットから出るパケットすべてに付く。
家庭の Wi-Fi のアクセスポイントと、送る側の PC の Wi-Fi は、この値を見て WMM の 4 つの待ち行列（音声・映像・普通・背景。RFC 8325）のどれで送るかを決める。
音声の印（EF = 46）が付いていれば、同じ Wi-Fi で動画やダウンロードが流れていても、音声のパケットが先に送られ、揺れと欠けが減る見込みがある。
インターネットの途中では消されることが多いので、効くのは主に家の中の区間になる。jamuru は低い遅延が最優先で、この印は遅延を増やさずに揺れを減らす手段になる。

これまで音声の UDP ソケットには何も付けていなかった。

## Decision

### 1. 音声のソケットに EF（46）を付ける

付けるのはセッションの音声のソケット 1 本（全員のリンクが共有するソケット。ADR-093）。シグナリング（WebSocket）や更新の確認には付けない。
値は WebRTC の音声の推奨（RFC 8837）と同じ EF（46。ToS / Traffic Class のバイトでは 0xB8）にする。

OS ごとの付け方:

| OS | 付け方 | 理由 |
|----|--------|------|
| Linux | `IP_TOS` と `IPV6_TCLASS` を 0xB8、`SO_PRIORITY` を 6 | IPv6 のソケットは IPv4 の相手へ IPv4 の見出しで送るので、両方に付ける。`SO_PRIORITY` は手元の Wi-Fi ドライバの待ち行列に載せる（0〜6 は特権が要らない） |
| macOS | `SO_NET_SERVICE_TYPE` を `NET_SERVICE_TYPE_VO` | DSCP と Wi-Fi の待ち行列の両方が音声になる（Apple の推奨）。拒まれたときだけ `IP_TOS` / `IPV6_TCLASS` |
| Windows | qWAVE（`QOSCreateHandle` → `QOSAddSocketToFlow` に `QOSTrafficTypeVoice`、`QOS_NON_ADAPTIVE_FLOW`） | `setsockopt(IP_TOS)` は既定で無視される。DSCP を手で指定する `QOSSetOutgoingDSCPValue` は管理者の権限が要るので使わない |

Windows の qWAVE は、接続していない UDP ソケットでは送り先のアドレスごとにフローを作る。そこで、あるアドレスへ最初のパケットを送るときにフローを足す。
足す作業は qWAVE のサービスへの問い合わせで数ミリ秒かかることがあり、音声の受信と同じスレッドでは待たないので、専用のスレッドで行う。足し終わるまでの最初のパケットは印なしで出る。

付けられなかったとき（OS が対応しない・拒まれた・qWAVE が使えない）は、ログに理由を残し、印なしで音声を送り続ける。印は要望であって、音声の送受信の条件にしない。

### 2. 設定で切れる。既定はオン

印を付けたパケットを落とす回線がまれにあるため、設定 `voice_priority`（一般タブの「音声を優先して送る」）で切れるようにする。既定はオン。セッションの開始時にソケットへ付けるので、変更は次のセッションから効く。

### 3. 利用者の情報の送信は変わらない

パケットの中身も宛先も変えず、見出しの優先度の欄だけを変える。利用者の情報の送信・保存・見え方は変わらないので、プライバシーの説明（`docs/getting-started/privacy.md`）は直さない。

## Consequences

- 同じ Wi-Fi で他の通信が流れていても、音声が先に送られる。遅延（`roundtrip_total_ms`）と音質は、パケットの中身を変えないので変わらない。
- 印を消す・落とすネットワークでは、付けても付けなくても同じ（付けて悪くなる回線は設定で切る）。
- qWAVE は Windows の「Quality Windows Audio Video Experience」サービスに依存する。サービスが止まっているときは印なしになる。
- 付けた依存: Windows の `windows` crate に `Win32_NetworkManagement_QoS` の機能を足した。同じ crate・同じ版なので、ライセンスの許可リストと第三者ライセンスの表示は変わらない。

## 検証

Linux では、印を付けたソケットから送ったデータグラムを、受け取る側が `IP_RECVTOS` / `IPV6_RECVTCLASS` で読み、IPv4・IPv6 のどちらも 0xB8（EF）であること、付けないソケットでは 0 であることをテストにした（`src/network/priority.rs`）。
Windows・macOS では、ベータ版を入れた端末から LAN の Linux へ音声を送り、受け取る側の見出しの値を測る。結果は下の「測定」に残す。
