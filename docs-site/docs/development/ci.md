---
sidebar_position: 3
title: CI/CD
description: jamjamの継続的インテグレーション
---

:::note
このドキュメントは開発者向けの解説資料です。
正確な仕様・制約・判断は [docs-spec/](https://github.com/koedame/jamjam-client/tree/main/docs-spec) を参照してください。
:::

# CI/CD

jamjamの継続的インテグレーション（CI）と継続的デリバリー（CD）について説明します。

## 概要

GitHub Actions を使用して以下を自動化しています:

- コード品質の検証（コンパイル、テスト、lint）
- マルチプラットフォームビルド
- リリース成果物の自動生成

## ワークフロー構成

```mermaid
flowchart TD
    subgraph Triggers
        PR[Pull Request]
        Push[Push to main]
        Tag[Tag v*]
    end

    subgraph CI["CI Workflow"]
        Check[check]
        Test[test]
        Lint[lint]
    end

    subgraph Build["Build Workflow"]
        BuildWin[build-windows]
        BuildMac[build-macos]
        BuildLinux[build-linux]
    end

    subgraph Release["Release Workflow"]
        Prepare[decide the tag]
        Publish[build + GitHub Release]
        Cask[update Homebrew cask]
    end

    PR --> CI
    Push --> CI
    Push --> Build
    Tag --> Build
    Push --> Release
    Tag --> Release
    Prepare --> Publish --> Cask
```

## CI Workflow

Pull Request および main ブランチへの push 時に実行されます。

| ジョブ | 内容 |
|-------|------|
| check | `cargo check --all-targets` |
| test | `cargo test --all-targets` |
| lint | `cargo fmt --check` + `cargo clippy` |

### マージ要件

Pull Request を main にマージするには以下をすべて満たす必要があります:

- check 成功
- test 成功
- lint 成功（警告なし）

## Build Workflow

main ブランチへの push またはタグ作成時に実行されます。

| OS | runner | 成果物 |
|----|--------|--------|
| Windows | windows-latest | `.msi`, `.exe` |
| macOS | macos-latest | `.dmg`, `.app` |
| Linux | ubuntu-latest | `.AppImage`, `.deb` |

## Release Workflow

main ブランチへの push またはタグ作成時に実行されます。

| きっかけ | 公開されるもの | Homebrew |
|---------|---------------|----------|
| main への push | ベータ版（タグ `vX.Y.Z-beta.<実行番号>`、X.Y.Z は `src-tauri/tauri.conf.json` の版）。GitHub の pre-release になり "Latest" には載らない | `jamjam@beta` を更新 |
| `vX.Y.Z` のタグ | 正式版 | `jamjam` を更新 |
| 手動起動 | 何も公開しない（ビルドの予行演習） | 更新しない |

リリースのビルドは、更新用の成果物と署名（`.sig`）も作り、正式版だけ更新情報 `latest.json` を Release に添えます（[ADR-041](https://github.com/koedame/jamjam-client/blob/main/docs-spec/adr/ADR-041-self-update.md)）。正式版のタグ `vX.Y.Z` は、`src-tauri/tauri.conf.json` の版と同じでなければ、更新情報を作る段階で失敗します。先に版を上げてからタグを打ってください。

ベータ版のタグは main への push ごとに作られます。正式版を使う人には届きません（[インストール](../getting-started/installation.md)）。

## ローカルでのCI実行

### 手動検証

```bash
# フォーマットチェック
cargo fmt --check

# Lint
cargo clippy --all-targets -- -D warnings

# コンパイルチェック
cargo check --all-targets

# テスト
cargo test --all-targets
```

### act を使用したローカル実行

[act](https://github.com/nektos/act) を使用して GitHub Actions をローカルで実行できます。

```bash
# インストール（macOS）
brew install act

# CI ワークフロー実行
act push -W .github/workflows/ci.yml

# 特定ジョブのみ実行
act push -W .github/workflows/ci.yml -j lint

# ドライラン
act push -W .github/workflows/ci.yml -n
```

:::tip
act は Docker ベースのため、Linux ジョブのみ実行可能です。Windows/macOS ビルドはスキップされます。
:::

## キャッシュ戦略

ビルド時間短縮のため、以下をキャッシュしています:

| 対象 | キー |
|------|-----|
| `~/.cargo/registry` | `cargo-registry-{Cargo.lock hash}` |
| `~/.cargo/git` | `cargo-git-{Cargo.lock hash}` |
| `target/` | `cargo-target-{os}-{Cargo.lock hash}` |

## 関連情報

- [ビルド](/docs/development/building) - ローカルビルド方法
- [テスト](/docs/development/testing) - テスト実行方法
