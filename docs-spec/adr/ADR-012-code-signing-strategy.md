---
sidebar_label: ADR-012 Code Signing Strategy
sidebar_position: 12
---

# ADR-012: Code Signing Strategy

## Context

macOS/Windows/Linux向けにデスクトップアプリを配布する際、コード署名が必要になる。
署名なしのアプリはOSのセキュリティ機能（Gatekeeper、SmartScreen）により警告が表示され、ユーザー体験が低下する。

テストユーザー配布を早期に開始しつつ、将来的には一般ユーザー向けに署名済みアプリを配布したい。

## Decision

### 段階1: 署名なし配布（現在）

- GitHub Releases で署名なしバイナリを配布
- インストールガイドに警告回避手順を記載
- テストユーザーは手動で信頼設定を行う

### 段階2: コード署名実装（将来）

優先順位:
1. **macOS**: Developer ID Certificate + notarization
2. **Windows**: EV Code Signing Certificate
3. **Linux**: 署名不要（AppImageで配布）

#### macOS 署名要件

| 項目 | 内容 |
|------|------|
| 証明書 | Developer ID Application Certificate |
| 公証 | xcrun notarytool による notarization |
| entitlements | マイク・ネットワークアクセス権限 |

#### Windows 署名要件

| 項目 | 内容 |
|------|------|
| 証明書 | EV Code Signing Certificate |
| ツール | SignTool.exe (Windows SDK) |

### 段階3: 自動アップデート機能（[ADR-041](./ADR-041-self-update.md) で実装済み。段階 2 を待たずに入れた）

Tauri Updater を使用した自動アップデート機能を実装する。

| 項目 | 内容 |
|------|------|
| 方式 | Tauri Updater (tauri.conf.json の `plugins.updater`) |
| 配信サーバー | GitHub Releases または専用サーバー |
| 署名 | アップデートパッケージの署名検証 |
| 前提条件 | なし（更新パッケージの署名は OS のコード署名とは別。決定は ADR-041） |

**必要な設定:**
```json
{
  "plugins": {
    "updater": {
      "endpoints": ["https://releases.example.com/{{target}}/{{arch}}/{{current_version}}"],
      "pubkey": "..."
    }
  }
}
```

### 段階4: チャット機能（将来）

P2P接続上でテキストチャット機能を追加する。

| 項目 | 内容 |
|------|------|
| 方式 | WebRTC DataChannel |
| プロトコル | 既存のP2P接続を利用 |
| 機能 | テキストメッセージ、タイムスタンプ |

## Consequences

### Pros

- テストユーザー配布を即座に開始可能
- 段階的に署名対応を追加できる

### Cons

- 段階1ではユーザーに警告回避の手間がかかる
- macOS notarizationのCI設定が複雑

## Related

- [Release Workflow](../../.github/workflows/release.yml)
