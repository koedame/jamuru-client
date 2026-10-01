---
sidebar_position: 1
title: jamjam について
description: ミュージシャン向け低遅延（< 2ms）P2P音声通信アプリ
---

:::note
このドキュメントは開発者向けの解説資料です。
正確な仕様・制約・判断は [docs-spec/](https://github.com/koedame/jamjam-client/tree/main/docs-spec) を参照してください。
:::

# jamjam

ミュージシャン向け低遅延（< 2ms）P2P音声通信アプリ

## 概要

jamjamは、ミュージシャンがインターネット越しにリアルタイムでジャムセッションや遠隔レコーディングを行うためのアプリケーションです。

## 主な特徴

- **アプリ起因遅延 < 2ms**: zero-latencyモードで実現
- **P2P通信**: 中央サーバーを介さない直接通信
- **クロスプラットフォーム**: Windows / macOS / Linux 対応
- **高音質**: 最大96kHz/32bit float対応、非圧縮PCMがデフォルト

## 対象ユースケース

- オンラインジャムセッション
- 遠隔レコーディング
- リアルタイム演奏セッション

## 対応プラットフォーム

| プラットフォーム | 状態 |
|-----------------|------|
| Windows | 対応済 |
| macOS | 対応済 |
| Linux | 対応済 |
| iOS | 将来対応 |
| Android | 将来対応 |

## 実装状況

| マイルストーン | 機能 | 状態 |
|---------------|------|------|
| コア機能 | オーディオエンジン、UDPトランスポート、プロトコル、CLI | 完了 |
| ネットワーク拡張 | STUN、シグナリング、FEC、暗号化（通信相手の確認を除く） | 完了 |
| デスクトップGUI | Tauri UI、ミキサー、設定画面、チャット | 完了 |
| 低遅延の実測検証 | ジッタバッファ・PLC の配線、プリセット遅延バジェットの検証、自動再接続 | 完了 |
| マルチピア（メッシュ） | 3名以上のセッション、参加者ごとの遅延表示 | 未着手 |
| 録音・メトロノーム共有 | — | 未着手 |
| エフェクト、VST/CLAP プラグインホスト | — | 未着手 |

> 「未着手」の機能はコードベースに存在しません。実装状況の正は
> [docs-spec/traceability.md](https://github.com/koedame/jamjam-client/blob/main/docs-spec/traceability.md)
> （要求と検証の対応表）です。

## 次のステップ

- [インストール](/docs/getting-started/installation) - jamjamをインストールする
- [クイックスタート](/docs/getting-started/quick-start) - 最初のセッションを始める
- [ジッタバッファ](/docs/concepts/jitter-buffer) - 低遅延を実現する技術を理解する
