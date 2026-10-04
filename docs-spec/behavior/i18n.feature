# This specification is the source of truth. Sync implementation when changed.

Feature: 国際化
  jamuruはユーザーインターフェースの多言語表示に対応する

  Background:
    Given jamuruアプリケーションがインストールされている

  @REQ-I18N-101 @must
  Scenario: 初回起動時のシステム言語検出
    Given システムロケールが日本語（ja）に設定されている
    And config.tomlにlanguage設定が存在しない
    When ユーザーがjamuruを初めて起動する
    Then UIが日本語で表示される

  @REQ-I18N-102 @must
  Scenario: 設定画面での言語切替
    Given UIが英語で表示されている
    When ユーザーが言語設定で「日本語」を選択する
    Then UIが即座に日本語で表示される
    And config.tomlのlanguageが"ja"に更新される

  @REQ-I18N-103 @must
  Scenario: 翻訳キーが存在しない場合のフォールバック
    Given UI言語が日本語に設定されている
    And 翻訳キー"experimental.new_feature"がja.jsonに存在しない
    When UIがそのキーを表示しようとする
    Then 英語の翻訳が表示される
    And コンソールに警告が出力される

  @REQ-I18N-104 @should
  Scenario: 言語設定の永続化
    Given ユーザーが言語設定を日本語に変更している
    When ユーザーがjamuruを終了して再起動する
    Then UIが日本語で表示される

  @REQ-I18N-105 @must
  Scenario: 接続後の画面に他の言語が混ざらない
    Given UI言語が英語に設定されている
    When ユーザーがルームに入り、接続後の画面が表示される
    Then ミキサー・チャット・退室確認を含む全ての表示が英語である
    And 参加・退出の通知もUI言語で表示される
    And UI言語を日本語に切り替えると、同じ画面の全ての表示が日本語になる
