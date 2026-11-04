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
| Windows | `IP_TOS` と `IPV6_TCLASS` を 0xB8 | Windows 11（ビルド 26200）で、レジストリの `DisableUserTOSSetting` が無い既定のまま、`IP_TOS` を付けた UDP のパケットが 0xB8 で届くことを測った（下の「測定」）。管理者の権限は要らない |

Windows で qWAVE（`QOSCreateHandle` → `QOSAddSocketToFlow` に `QOSTrafficTypeVoice`）を使わない理由:

- 音声のソケットは IPv4 と IPv6 の両用の 1 本（ADR-093）。qWAVE は、IPv6 のソケットから IPv4 の宛先へのフローを作れない（`ERROR_INVALID_PARAMETER`）。IPv4-mapped の宛先も作れない（`ERROR_NOT_FOUND`）。IPv4 の相手には印を付けられない。
- フローは宛先ごとに作るため、最初のパケットが印なしで出て、宛先が変わるたびに足す仕組みが要る。
- 音声に付く値が 56（CS7）で、46 ではない。46 を指定する `QOSSetOutgoingDSCPValue` は管理者の権限が要る。

古い Windows で `IP_TOS` が無視されるかは測っていない。無視される場合は印なしで動く（害は無い）。

付けられなかったとき（OS が対応しない・拒まれた）は、ログに理由を残し、印なしで音声を送り続ける。印は要望であって、音声の送受信の条件にしない。

### 2. 設定で切れる。既定はオン

印を付けたパケットを落とす回線がまれにあるため、設定 `voice_priority`（一般タブの「音声を優先して送る」）で切れるようにする。既定はオン。セッションの開始時にソケットへ付けるので、変更は次のセッションから効く。

### 3. 利用者の情報の送信は変わらない

パケットの中身も宛先も変えず、見出しの優先度の欄だけを変える。利用者の情報の送信・保存・見え方は変わらないので、プライバシーの説明（`docs/getting-started/privacy.md`）は直さない。

## Consequences

- 同じ Wi-Fi で他の通信が流れていても、音声が先に送られる。遅延（`roundtrip_total_ms`）と音質は、パケットの中身を変えないので変わらない。
- 印を消す・落とすネットワークでは、付けても付けなくても同じ（付けて悪くなる回線は設定で切る）。
- 付けるのはソケットのオプション 1 つで、依存の追加は無い。パケットごとの処理も増えない。

## 検証

Linux では、印を付けたソケットから送ったデータグラムを、受け取る側が `IP_RECVTOS` / `IPV6_RECVTCLASS` で読み、IPv4・IPv6 のどちらも 0xB8（EF）であること、付けないソケットでは 0 であることをテストにした（`src/network/priority.rs`）。
Windows・macOS では、ベータ版を入れた端末から LAN の Linux へ音声を送り、受け取る側の見出しの値を測る。結果は下の「測定」に残す。

## 測定

### Windows でどの付け方が効くか（IPv4）

Windows 11（ビルド 26200、`DisableUserTOSSetting` は未設定）の PC から、同じ LAN の Linux へ UDP を 30 個ずつ送り、受ける側が `IP_RECVTOS` で IPv4 の見出しの ToS を読んだ。

| 送る側のソケットと付け方 | 受けた ToS（30 個中） |
|---|---|
| IPv4 ソケット、付けない | 0x00 |
| IPv4 ソケット、`IP_TOS` = 0xB8 | 0xB8（30） |
| 両用ソケット、`IP_TOS` = 0xB8、IPv4 の宛先 | 0xB8（30） |
| IPv4 ソケット、qWAVE の音声フロー（IPv4 の宛先） | 0xE0 = 56（30） |
| 両用ソケット、qWAVE（IPv4 の宛先） | フローを作れず（`ERROR_INVALID_PARAMETER`）、0x00 |
| 両用ソケット、qWAVE（IPv4-mapped の宛先） | フローを作れず（`ERROR_NOT_FOUND`）、0x00 |
| 両用ソケット、qWAVE（宛先なし） | フローを作れず（`ERROR_INVALID_PARAMETER`）、0x00 |

`IPV6_TCLASS` を Windows で付けるところまでは通った（エラーなし）が、受ける側で読む IPv6 の経路が手元に無く、届いた値は測っていない。
