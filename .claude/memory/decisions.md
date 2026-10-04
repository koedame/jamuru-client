# decisions.md - 意思決定メモ

> セッション中の小さな決定を記録する。
> 重要な決定は `docs-spec/adr/` にADRとして昇格する。

## フォーマット

```markdown
### YYYY-MM-DD: 決定タイトル

**状況**: なぜこの決定が必要だったか
**決定**: 何を決定したか
**理由**: なぜその選択をしたか
```

---

## 決定履歴

### 2025-01-15: Core Library Architecture の明文化

**状況**: CLI と GUI でコードが重複しているように見え、共通化すべきか検討が必要だった
**決定**: 現状の3層アーキテクチャ（Interface Layer / Core Library）を維持。セッションオーケストレーションは各インターフェースに残す
**理由**:
- コアロジック（audio/network/protocol）は既に共通化済み
- オーケストレーション層は制御フローが異なる（CLI: 同期的, GUI: 非同期IPC）
- 2つのインターフェースでは抽象化のコストがメリットを上回る
- 詳細: [ADR-011](../../docs-spec/adr/ADR-011-core-library-architecture.md)

### 2026-07-19: glib Dependabot alert は upstream 待ちで放置

**状況**: `src-tauri/Cargo.lock` の `glib 0.18.5`（GHSA-wrw7-89jp-8q8g, medium）が Dependabot で検出された。`glib::VariantStrIter` の NULL ポインタ参照によるクラッシュで、fixed version は `0.20.0`
**決定**: Cargo 依存関係の更新では修正しない。alert はオープンのまま放置し、次回 `/sync-spec` 等での定期チェック時に再確認する
**理由**:
- `glib` は jamuru のコードから直接使用しておらず、Tauri 2.11.5（最新）→ wry 0.55.1 → tao/muda → `gtk 0.18.2`（gtk3-rs）が固定した推移的依存
- `gtk` クレート自体が upstream で UNMAINTAINED（gtk3-rs、0.18.2 が最終リリース）であり、`glib >= 0.20` を要求する新バージョンが存在しないため `cargo update` では一切バージョンを上げられない（dry-run で確認済み）
- Tauri が Linux バックエンドを gtk3-rs/webkit2gtk スタックから移行しない限り修正不可能な upstream 側の課題
- 該当コード（`VariantStrIter`、GVariant 文字列配列のイテレータ）は jamuru のコードパスから到達しないため、実害は低いと判断

---

<!-- 新しい決定はここに追加 -->
