# AGENTS.md - AIエージェント向け開発ガイド

このファイルは AI コーディングエージェント向けのプロジェクトガイドである。
Claude Code は `CLAUDE.md` からのインポート（`@AGENTS.md`）で本ファイルを読み込む。

## プロジェクト概要

**jamjam** — P2P音声通信アプリ（macOS / Windows / Linux ネイティブ動作）

- コア: Rust（音声 I/O、ネットワーク、プロトコル）
- GUI: Tauri + React/TypeScript（`src-tauri/` + `ui/`）
- バイナリ: `jamjam`（CLI）
- シグナリングサーバーはこのリポジトリに含まれない（別に運用している）

## 最優先要件

**低遅延と音質は最優先事項である。**

音楽セッション用途では30ms以上の遅延で演奏の心地よさを失う。
このため、以下を設計の最優先事項とする:

- アプリ起因の遅延を限りなく0msに近づける（目標: 片道 < 2ms）
- 遅延 ≒ ネットワークRTT となることを目指す
- 音質は非圧縮PCM（32-bit float）をデフォルトとする
- 帯域効率より遅延削減を優先する
- 日本国内光回線（RTT 10-25ms）での使用を主な対象とする

これらの要件は他の設計判断より優先される。詳細は [ADR-008](./docs-spec/adr/ADR-008-zero-latency-mode.md) を参照。

## ドキュメント体系

```
CLAUDE.md            # Claude Code 設定（本ファイルをインポート）
AGENTS.md            # AIエージェント向け開発ガイド（本ファイル）
Plans.md             # タスク管理

docs-spec/           # 仕様書（実装の唯一の正）
├── architecture.md      # 技術構成（最重要）
├── requirements.md      # 要求仕様（V字左辺の最上位・REQ-ID 定義）
├── traceability.md      # 要求と検証の対応表（自動生成）
├── ui-ux-guideline.md   # UI/UXガイドライン
├── adr/                 # 設計判断記録（ADR）
├── api/                 # API境界定義
├── behavior/            # 振る舞い定義（BDD/Gherkin・@REQ-ID タグ付き）
└── ui/                  # UI仕様（画面・コンポーネント・デザイントークン）

docs-site/           # Docusaurus 開発者向けドキュメント（解説資料。仕様ではない）

.claude/
├── settings.json    # 権限設定
├── rules/           # トピック別ルール（下記「ルール一覧」参照）
├── skills/          # スキル定義（ワークフロー手順）
└── memory/          # セッション間メモ
    ├── decisions.md # 意思決定メモ
    └── patterns.md  # 再利用パターン
```

## 開発フロー

反復V字モデル（W字）を採用する。Plans.md の 1 作業項目が 1 つの V に対応し、左辺の各成果物は対応する検証層を持つ（[ADR-018](./docs-spec/adr/ADR-018-iterative-v-model-traceability.md)）。連番のフェーズ番号は使わない（[ADR-023](./docs-spec/adr/ADR-023-drop-phase-as-identifier.md)）。

```
1. Plan    : プランモードで作業計画を立て、Plans.md にタスクを記録
2. Spec    : 要求に REQ-ID を振る（behavior/*.feature のタグ or requirements.md）
3. Work    : Plans.md のタスクを仕様（docs-spec/）に従って実装
4. Verify  : テストに `Verifies: REQ-XXX-NNN` を付け、品質チェックを実行
5. Review  : /code-review でコード品質チェック（仕様書は /spec-review）
6. Commit  : /commit でコミット作成
```

| V字 左辺 | 対応する検証層（V字 右辺） |
|---------|--------------------------|
| `requirements.md`（要求） | `behavior/*.feature` + 全層のテスト |
| `architecture.md` / ADR（基本設計） | システムE2E（`tests/e2e/`） |
| `api/*.md` / `ui/components/*.md`（詳細設計） | 結合テスト（`tests/*.rs`）・UI単体（`ui/src/**/*.test.ts`） |
| `ui/screens/*.md`（GUI の操作と状態） | GUI E2E（`tests/e2e/tests/gui.rs`、実アプリを起動。ADR-025） |
| `src/` `src-tauri/` `ui/`（実装） | 単体テスト（`#[cfg(test)]`） |

未検証の要求は [docs-spec/traceability.md](./docs-spec/traceability.md) に列挙される。`must` 要求が未検証だと `cargo test` が失敗する。

## 主要スキル

| スキル | 用途 |
|--------|------|
| `/commit` | 品質チェック + コミットメッセージ生成 + コミット |
| `/code-review` | コードレビュー（本質対応・仕様準拠・テスト品質・OSS適合） |
| `/spec-review` | 仕様書・開発者ドキュメントのレビュー |
| `/sync-spec` | 仕様と実装の同期チェック |

## ルール一覧（.claude/rules/）

トピック別の開発ルール。Claude Code は各ファイルの `paths:` frontmatter に基づき対象ファイル操作時に自動読み込みする。**他の AI エージェントは自動読み込みされないため、該当する作業の前に対応するルールを必ず読むこと。**

| ルール | 内容 | 対象 |
|--------|------|------|
| `git-workflow.md` | コミット・プッシュ規約・ブランチ運用（Git-Flow） | 常時 |
| `defect-response.md` | 本質的な欠陥はスコープ外でも即対応する基準・記録に留める基準 | 常時 |
| `traceability.md` | 要求ID付与・検証宣言・対応表更新 | 仕様・コード・テスト編集時 |
| `source-available.md` | Source Available ライセンス・秘匿情報ポリシー | 常時 |
| `spec-sync.md` | 仕様書と実装の同期・実装フェーズ指示 | コード・仕様・依存関係編集時 |
| `docs-authoring.md` | 仕様書の書き方・図解ルール | docs-spec/, docs-site/, docs/ 編集時 |
| `implementation-quality.md` | 実装品質（形骸化実装禁止等） | コード編集時 |
| `test-quality.md` | テスト品質（テスト削除禁止等） | コード・テスト編集時 |
| `ui-component-rules.md` | UIコンポーネント設計（Pure + Adapter） | ui/src/ 編集時 |
| `ci-cd.md` | CI/ローカル整合・GitHub機能ルール | CI・ビルド設定編集時 |

## 主要コマンド

```bash
# 品質チェック（すべてパスしないとコミット不可）
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                             # トレーサビリティ検証を含む
cd ui && npm run test:run              # UI単体テスト（i18n要求はここで検証）

# 要求と検証の対応表を再生成（要求・テストを増減させたら必須）
JAMJAM_UPDATE_TRACEABILITY=1 cargo test --test traceability_test

# ビルド
cargo build                # コアライブラリ + CLI
cargo build --release      # リリースビルド

# Tauri GUI（必ずプロジェクトルートから実行。ADR-009参照）
cargo tauri dev
JAMJAM_SERVER_URL=https://<サーバー> cargo tauri build   # 接続先はソースに無い（ADR-030）
```

## 仕様書ルール

1. **docs-spec/ が実装の唯一の正**
   - 実装が仕様と矛盾する場合、仕様に合わせて実装を修正
   - 仕様変更が必要な場合は新規ADRを作成
2. **ADRは追加のみ**
   - 既存ADRの変更は原則禁止
   - 決定を変更する場合は新規ADRで上書き
3. **同期ルール**
   - 実装変更時は同一コミット（または直後のコミット）で仕様書も更新
   - `/sync-spec` で定期的に整合性チェック

詳細は [.claude/rules/spec-sync.md](./.claude/rules/spec-sync.md) を参照。

## 対話・作業ルール

### 質問・提案時のルール

- 質問や提案をするときは、要件から判断した推奨案を必ず添える
- 推奨理由を明示する（なぜその案が最適か）
- 選択肢を提示する場合は、推奨案を最初に置く

❌ 悪い例

```
どちらを修正しますか？
- 仕様を修正
- 実装を修正
```

⭕ 良い例

```
どちらを修正しますか？
- 実装を修正（推奨）: 仕様書が正であり、ADR-002で決定済みのため
- 仕様を修正: 実装の設計がより実用的な場合
```

### 並列作業の効率化

- 独立した作業は並列で実行する（ファイル読み込み、検索、エージェント起動など）
- コンフリクトの可能性がある作業は順序を制御する
- 依存関係を考慮して最適な実行順序を判断する

並列実行の判断基準:

| 状況 | 実行方法 |
|-----|---------|
| 複数ファイルの読み込み | 並列 |
| 複数の独立した検索 | 並列 |
| 同一ファイルの読み込み→編集 | 順次 |
| 複数ファイルの編集（依存なし） | 並列 |
| 複数ファイルの編集（依存あり） | 順次 |
| ビルド→テスト | 順次 |
| 複数エージェントでの調査 | 並列 |

❌ 悪い例

```
# 1つずつ順番に読み込み
Read file A
Read file B
Read file C
```

⭕ 良い例

```
# 並列で読み込み
Read file A, B, C simultaneously
```

## 言語規約

- ソースコードのコメント: 英語
- コミットメッセージ: 英語
- ドキュメント・ユーザーとの会話: 日本語

## セッションメモ（.claude/memory/）

自動では読み込まれない。必要になったときに明示的に参照・追記する。

- `decisions.md` — セッション中の小さな決定を記録。重要な決定はADRに昇格
- `patterns.md` — コードパターン、ベストプラクティス

## 参照

- [architecture.md](./docs-spec/architecture.md) - 技術構成
- [requirements.md](./docs-spec/requirements.md) - 要求仕様（REQ-ID 定義）
- [traceability.md](./docs-spec/traceability.md) - 要求と検証の対応表
- [ADR一覧](./docs-spec/README.md#adr設計判断記録) - 設計判断
- [API仕様](./docs-spec/README.md#api仕様) - API境界
