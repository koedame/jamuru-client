# This specification is the source of truth. Sync implementation when changed.

Feature: 音声品質
  jamjamでの音声品質に関する振る舞い

  Background:
    Given jamjamアプリケーションが起動している
    And オーディオデバイスが正常に認識されている
    And セッションに接続済み

  # コーデック選択
  # 狭帯域回線は対象外（ADR-022）。全プリセットが非圧縮PCMを使用し、
  # 帯域に応じたコーデック切替は行わない。カスタムプリセットの保存も対象外。
  @REQ-AUD-101 @must
  Scenario: 非圧縮PCMコーデックを使用する
    Given ネットワーク帯域が10Mbps以上
    When コーデックを「非圧縮PCM」に設定する
    Then 音声は圧縮されずに送信される
    And 送信ビットレートは約1.5Mbps/chになる
    And コーデック起因の遅延は0msになる

  # サンプルレート
  @REQ-AUD-104 @must
  Scenario: サンプルレート48kHzで動作する
    When サンプルレートを「48000Hz」に設定する
    Then オーディオエンジンは48kHzで動作する
    And 相手にも48kHzで音声が伝送される

  @REQ-AUD-105 @must
  Scenario: サンプルレート96kHzで動作する
    Given オーディオインターフェースが96kHz対応
    When サンプルレートを「96000Hz」に設定する
    Then オーディオエンジンは96kHzで動作する
    And 相手にも96kHzで音声が伝送される

  @REQ-AUD-106 @must
  Scenario: 異なるサンプルレートの参加者がいる場合
    Given 参加者Bが48kHzに設定している
    And 参加者Aが96kHzに設定している
    When セッションが開始される
    Then 参加者Aの音声は48kHzにリサンプリングされる
    And 参加者Bの音声は参加者Aに48kHzで送信される

  # チャンネル
  @REQ-AUD-107 @must
  Scenario: モノラル入力で動作する
    When 送信チャンネルを「モノラル」に設定する
    Then 入力から1チャンネルを取り込み、パンを適用した上で送信される
    And 受信側では、パンが中央なら両チャンネル同一で再生される

  @REQ-AUD-108 @must
  Scenario: ステレオ入力で動作する
    When 送信チャンネルを「ステレオ」に設定する
    Then 入力から左右2チャンネルを取り込み、混ぜずに送信される
    And 受信側ではステレオで再生される
    # 入力デバイスが1チャンネルしか開けないときはモノラルで取り込み、相手にもモノラルと伝える（ADR-038）

  @REQ-AUD-117 @must
  Scenario: 相手の送信チャンネル数をミキサーに表示する
    Given 参加者Bが送信チャンネルを「モノラル」に設定している
    When 参加者Aと参加者Bがセッションに接続する
    Then 参加者Aのミキサーは参加者Bのチャンネルを「モノラル」と表示する

  @REQ-AUD-118 @must
  Scenario: 多チャンネルのインターフェースで、選んだ入力チャンネルを取り込む
    Given 入力デバイスが8チャンネルを持つ
    When 入力チャンネルを「5」と「6」に設定する
    Then デバイスの5番と6番のチャンネルが取り込まれて送信される
    And 選んでいないチャンネルの音は送信されない
    # 選んだチャンネルを持たないデバイスでは、先頭のチャンネルを取り込む

  @REQ-AUD-119 @must
  Scenario: 多チャンネルのインターフェースで、選んだ出力チャンネルに再生する
    Given 出力デバイスが8チャンネルを持つ
    When 出力チャンネルを「5」と「6」に設定する
    Then 受信した音の左右がデバイスの5番と6番のチャンネルに再生される
    And 選んでいないチャンネルは無音になる
    # 選んだチャンネルを持たないデバイスでは、先頭の2チャンネルに再生する

  @REQ-AUD-120 @must
  Scenario: 受信側のパンは、届いた音がステレオかモノラルかで意味が変わる
    Given 参加者Bが送信チャンネルを「ステレオ」に設定し、左だけに音がある
    When 参加者Aが参加者Bのパンを中央にする
    Then 左の音は左だけから同じ大きさで再生され、右は無音のままである
    And パンを右に振ると、右はそのままで左だけが小さくなる
    # 参加者Bが「モノラル」なら、Bが自分のパンで置いた左右をAの等パワーのパンが動かす（従来どおり）

  # フレームサイズ
  @REQ-AUD-109 @must
  Scenario: フレームサイズ64サンプルで動作する
    When フレームサイズを「64 samples」に設定する
    Then オーディオバッファは64サンプル（約1.33ms @ 48kHz）になる
    And フレームサイズ起因の遅延は約1.33msになる

  @REQ-AUD-110 @must
  Scenario: フレームサイズ256サンプルで動作する
    When フレームサイズを「256 samples」に設定する
    Then オーディオバッファは256サンプル（約5.33ms @ 48kHz）になる
    And バッファアンダーランが発生しにくくなる

  # ローカルモニタリング
  @REQ-AUD-111 @must
  Scenario: ローカルモニタリングを有効にする
    When ローカルモニタリングを「ON」に設定する
    Then 自分の音声がネットワーク遅延なしで聞こえる
    And 他の参加者の音声も同時に聞こえる

  @REQ-AUD-112 @must
  Scenario: ローカルモニタリングを無効にする
    When ローカルモニタリングを「OFF」に設定する
    Then 自分の音声は直接聞こえない
    And 他の参加者の音声のみ聞こえる

  # 音声処理なし
  @REQ-AUD-113 @must
  Scenario: 音声がピュアに伝送される
    Given 音声処理（AEC、NS、AGC）が無効
    When 楽器（ギター）を演奏する
    Then 音声は一切の処理なしで伝送される
    And 受信側では演奏したままの音が聞こえる

  # 全プリセットは非圧縮PCMを使用する。Opus はどのプリセットのフレームサイズでも
  # 符号化できないため（ADR-021）。FEC の冗長度はジッタバッファ段数から導出される
  # （グループサイズ = 段数）。
  @REQ-AUD-114 @must
  Scenario: ultra-low-latencyプリセットを使用する
    When プリセット「ultra-low-latency」を選択する
    Then コーデックが「非圧縮PCM」に設定される
    And フレームサイズが「64 samples」に設定される
    And Jitterバッファが「最小（1フレーム）」に設定される
    And FECが「OFF」に設定される

  @REQ-AUD-115 @must
  Scenario: balancedプリセットを使用する
    When プリセット「balanced」を選択する
    Then コーデックが「非圧縮PCM」に設定される
    And フレームサイズが「128 samples」に設定される
    And Jitterバッファが「4フレーム」に設定される
    And FECが「ON（グループ4 = 冗長度25%）」に設定される

  @REQ-AUD-116 @must
  Scenario: high-qualityプリセットを使用する
    When プリセット「high-quality」を選択する
    Then コーデックが「非圧縮PCM」に設定される
    And フレームサイズが「256 samples」に設定される
    And Jitterバッファが「8フレーム」に設定される
    And FECが「ON（グループ8 = 冗長度12.5%）」に設定される

