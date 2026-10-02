<!-- このドキュメントは実装の正です。変更時は実装も同期すること -->

# ADR-007: i18nライブラリ選定

## Context

jamjamは複数プラットフォーム向けGUIを提供する:
- デスクトップ: Tauri 2.0 + TypeScript
- モバイル: Flutter + Dart（段階5）

国際化の要件:
- 日本語と英語の初期対応
- プラットフォーム間で翻訳キー構造を統一
- 型安全な翻訳アクセス
- i18nライブラリのバンドルサイズ < 50KB

候補ライブラリ:

| ライブラリ | プラットフォーム | サイズ | 型安全性 |
|-----------|----------------|--------|----------|
| i18next | Tauri | 約28KB | プラグイン経由 |
| typesafe-i18n | Tauri | 約5KB | ネイティブ |
| flutter_localizations | Flutter | 組み込み | ネイティブ |
| easy_localization | Flutter | 約15KB | 部分的 |

## Decision

- デスクトップ（Tauri）: i18next + i18next-browser-languageDetector
- モバイル（Flutter）: flutter_localizations + intl

選定理由:
- i18nextは最も成熟したエコシステムを持つ
- flutter_localizationsはFlutter公式の標準的アプローチ
- JSON/ARBファイルで翻訳キー構造を共有可能

## Consequences

利点:
- 大規模コミュニティによるサポート
- 豊富なドキュメント
- 補間・複数形対応

欠点:
- i18nextはtypesafe-i18nより大きい（28KB vs 5KB）
- プラットフォーム間で翻訳ファイル形式が異なる（JSON vs ARB）
