# This specification is the source of truth. Sync implementation when changed.

Feature: 遅延管理
  jamjamでの遅延に関する振る舞い

  Background:
    Given jamjamアプリケーションが起動している
    And オーディオデバイスが正常に認識されている
    And セッションに接続済み

  # 遅延表示
  @REQ-LAT-101 @must
  Scenario: 遅延情報を表示する
    Given 接続が確立している
    Then 以下の遅延情報が表示される:
      | 項目 | 説明 |
      | ネットワークRTT | 相手との往復遅延（ms） |
      | Jitterバッファ | 現在のバッファサイズ（ms） |
      | 総遅延 | 片道の推定総遅延（ms） |

  # メッシュ構成が前提のため未検証（Plans.md「実環境待ち」の REQ-LAT-102）。
  @REQ-LAT-102 @should
  Scenario: 複数参加者の遅延を個別表示する
    Given 3名でセッション中
    Then 各参加者との遅延が個別に表示される
    And 最も遅延が大きい参加者がハイライトされる

  # ジッターモニタリングと推奨設定
  @REQ-LAT-103 @must
  Scenario: ジッターをリアルタイムで表示する
    Given 接続が確立している
    Then 各参加者のジッター値がリアルタイムで表示される
    And 表示は1秒ごとに更新される
    And 以下の情報が表示される:
      | 項目 | 説明 |
      | RTT | 往復遅延（ms） |
      | Jitter | 到着時刻の揺らぎ（ms） |
      | Packet Loss | パケットロス率（%） |

  @REQ-LAT-104 @must
  Scenario: ジッターが小さい場合にzero-latencyを推奨する
    Given 相手とのジッターが0.5msで安定している
    And パケットロス率が0.1%未満
    Then 接続品質が「非常に良好」と表示される
    And 「zero-latencyモード推奨」と表示される
    And [zero-latencyに切り替え]ボタンが表示される

  @REQ-LAT-105 @must
  Scenario: ジッターが大きい場合にバッファ増加を推奨する
    Given 相手とのジッターが15msで不安定
    And 現在のプリセットが「zero-latency」
    Then 接続品質が「悪い」と表示される
    And 「不安定。バッファを増やしてください」と警告が表示される
    And 推奨プリセット「balanced」への切り替えボタンが表示される

  # 設定側（プリセット適用とジッタバッファ再構成）を検証する。
  # ボタン表示と通知トーストは UI 側の結線であり未実施。
  @REQ-LAT-106 @must
  Scenario: 推奨に従ってプリセットを切り替える
    Given 相手とのジッターが0.8msで安定している
    And 現在のプリセットが「balanced」
    When [zero-latencyに切り替え]ボタンをクリックする
    Then プリセットが「zero-latency」に変更される
    And Jitterバッファがパススルーモードになる
    And 「設定を変更しました」と通知される

  @REQ-LAT-107 @must
  Scenario: 接続品質が変化した場合に通知する
    Given 相手とのジッターが1ms未満で安定している
    And 接続品質が「非常に良好」
    When ネットワーク状況が悪化してジッターが10msを超える
    Then 接続品質が「悪い」に変化する
    And 「接続品質が低下しました」と警告が表示される
    And 推奨設定が「balanced」に変更される

  # Jitterバッファ
  @REQ-LAT-108 @must
  Scenario: Jitterバッファが適応的に調整される
    Given Jitterバッファが「適応的」に設定されている
    And 初期バッファサイズが4フレーム
    When ネットワークジッターが増加する
    Then Jitterバッファサイズが自動的に増加する
    And 「バッファサイズを調整しました」と通知される

  @REQ-LAT-109 @must
  Scenario: Jitterバッファが最小サイズを下回らない
    Given Jitterバッファの最小サイズが2フレームに設定されている
    When ネットワークが安定している
    Then Jitterバッファは2フレーム以下にならない

  @REQ-LAT-110 @must
  Scenario: Jitterバッファを手動で設定する
    When Jitterバッファを「固定: 3フレーム」に設定する
    Then Jitterバッファサイズは常に3フレームになる
    And 自動調整は行われない

  @REQ-LAT-111 @must
  Scenario: Jitterバッファをパススルーモードで使用する
    Given プリセット「zero-latency」を使用
    When Jitterバッファが「パススルー」に設定されている
    Then パケットは受信後即座に再生される
    And Jitterバッファによる遅延は0msになる
    And ネットワークジッターは音声の乱れとして直接現れる

  # パケットロス
  @REQ-LAT-112 @must
  Scenario: パケットロスが発生してもFECで復元される
    Given FECが有効（冗長度10%）
    When 5%のパケットロスが発生する
    Then FECにより大部分のパケットが復元される
    And パケット復元率は90%以上になる

  @REQ-LAT-113 @must
  Scenario: パケットロス率が高い場合
    Given FECが有効（冗長度10%）
    When 20%のパケットロスが発生する
    Then FECでは復元できないパケットが発生する
    And 補間（PLC）により急激な音の途切れを防ぐ
    And 「パケットロス率が高くなっています」と警告が表示される

  @REQ-LAT-114 @must
  Scenario: FECが無効の場合のパケットロス
    Given FECが無効
    When 5%のパケットロスが発生する
    Then ロスしたパケットは補間（PLC）で処理される
    And 直前の音声が減衰してフェードアウトする
    And クリックノイズ（バツッという音）は発生しない

  # 遅延目標
  @REQ-LAT-115 @must
  Scenario: zero-latencyモードでの国内光回線セッション
    Given 光回線同士（日本国内）で接続
    And プリセット「zero-latency」を使用
    And ネットワークRTTが20ms
    Then アプリケーション起因の片道遅延は2ms以下
    And 総片道遅延は12ms以下
    And 音楽セッションに適した遅延である

  @REQ-LAT-116 @must
  Scenario: LAN環境での遅延
    Given 同一LAN内の2台で接続
    And プリセット「ultra-low-latency」を使用
    And ネットワークRTTが1ms以下
    Then アプリケーション起因の片道遅延は5ms以下
    And 総片道遅延は6ms以下

  # 上限は ADR-019 のバジェット（設計値 16ms + 余裕 2ms）。
  # ADR-008 の概算 15ms は 128samples/4フレームというパラメータ確定前の見積もりであり、
  # 実パラメータでは capture 2.67ms + jitter 10.67ms + playback 2.67ms = 16.00ms となる。
  @REQ-LAT-117 @must
  Scenario: インターネット環境での遅延
    Given インターネット越しに接続
    And プリセット「balanced」を使用
    And ネットワークRTTが50ms
    Then アプリケーション起因の片道遅延は18ms以下
    And 総片道遅延（ネットワーク込み）は約41ms

  # 帯域
  # ビットレート適応は対象外（ADR-022）。非圧縮PCMのビットレートはサンプルレートと
  # ビット深度で決まり、遅延を増やさずに下げることはできない。不足の検出と警告は行う。
  #
  # 判定は「受信したレートが必要帯域に届くか」ではなく「相手が送った音声パケットの
  # うち届かなかった割合」で行う（ADR-046）。相手は自分のプリセットの必要量ちょうどしか
  # 送らないので、健全な回線でも受信レートは必要帯域を超えられず、レートで比べると
  # 健全な回線が常に境界線上に載ってしまう。相手の連番は、相手が送った数を教える。
  @REQ-LAT-124 @must
  Scenario: 帯域が不足した場合に警告する
    Given 相手が音声パケットを送っている
    When 直近の測定区間で、相手が送った音声パケットの5%以上が届かない
    Then 「帯域が不足しています」と警告される
    And ビットレートは変更されない

  @REQ-LAT-125 @must
  Scenario: 帯域の余裕が少ない場合に警告する
    Given 相手が音声パケットを送っている
    When 直近の測定区間で、相手が送った音声パケットの1%以上5%未満が届かない
    Then 余裕が少ないことが警告される

  @REQ-LAT-126 @must
  Scenario: 帯域を測定していない間は警告しない
    Given 接続直後で測定区間が完了していない
    Then 帯域に関する警告は表示されない

  @REQ-LAT-127 @must
  Scenario: 受信が途絶えたら帯域警告でなく無通信を示す
    Given 直前の測定区間では帯域が「不足」または「余裕なし」と判定されていた
    When 直近の測定区間で受信バイト数が増えない
    Then 「相手からパケットが届いていない」と表示される
    And 直前の帯域判定は表示されない

  # Wi-Fi のような回線では、健全でも1区間に数個のパケットが遅れて欠けることがある。
  # 接続直後は特に、返信を意図的に遅らせるテスト用の相手や接続確立そのものにより、
  # 最初の数秒はkeepalive程度のごく僅かな受信しかない。
  @REQ-LAT-128 @must
  Scenario: 接続してから数秒は帯域警告を保留する
    Given 接続してからまだ起動猶予期間を過ぎていない
    When その間の測定区間が「不足」または「余裕なし」と判定される
    Then 帯域に関する警告は表示されない

  @REQ-LAT-129 @must
  Scenario: 帯域不足の警告は複数区間続けて確認してから表示する
    Given 起動猶予期間を過ぎている
    When 測定区間が連続して「不足」または「余裕なし」と判定される
    Then 既定の連続回数に達するまで警告は表示されない
    And 既定の連続回数に達したら警告が表示される
    And その後1回でも「十分」と判定されたら警告は即座に消える

  @REQ-LAT-131 @must
  Scenario: 相手が音声を送っていない間は帯域不足にしない
    Given 相手が自分のマイクをミュートし、音声パケットを送っていない（keepalive だけが届く）
    When 測定区間が完了する
    Then 帯域に関する警告は表示されない

  # 接続品質
  # 判定は core library（`src/network/quality.rs`）が行い、UI は色分けのみを担う。
  # 閾値をUI側で再実装してはならない。
  @REQ-LAT-121 @must
  Scenario: 接続品質インジケーターの表示
    Then 接続品質インジケーターが表示される
    And インジケーターは以下の状態を示す:
      | 状態 | 条件 |
      | 良好（緑） | RTT < 30ms、パケットロス < 1% |
      | 普通（黄） | RTT < 100ms、パケットロス < 5% |
      | 悪い（赤） | RTT >= 100ms または パケットロス >= 5% |

  # RTTは0.0msで初期化されるが、0.0msは「良好」の範囲内の実測値でもあるため、
  # 初期値と最初の測定結果を区別できないと接続直後に誤って「良好」と判定される。
  @REQ-LAT-130 @must
  Scenario: RTTを測定するまで接続品質を判定しない
    Given 接続した直後でRTTを一度も測定していない
    Then 接続品質インジケーターは色を示さない
    And ログに接続品質は記録されない
    When 最初のRTT測定値を受信する
    Then その測定値で接続品質が判定される

  # オーディオデバイス遅延
  @REQ-LAT-122 @must
  Scenario: オーディオデバイスの遅延を表示する
    Given オーディオインターフェースが接続されている
    Then デバイスの入出力遅延が表示される
    And 例: 「入力: 3ms、出力: 3ms」

  @REQ-LAT-123 @should
  Scenario: ASIO使用時の低遅延
    Given Windows環境
    And ASIO対応オーディオインターフェースが接続されている
    When ASIOドライバを選択する
    Then デバイス遅延は3ms以下になる（WASAPI: 典型的に10ms以上）
