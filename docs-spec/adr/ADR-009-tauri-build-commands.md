# ADR-009: Tauri Build Commands Cross-Platform Strategy

## Context

Tauriの`beforeDevCommand`/`beforeBuildCommand`は、実行環境によって異なるディレクトリから実行される：

- **CI環境（GitHub Actions）**: `working-directory`設定により動作が変わる
- **ローカル環境**: `cargo tauri`を実行したディレクトリに依存

この差異により、CIでは成功するがローカルでは失敗する、またはその逆の問題が発生した。

### 発生した問題

1. `cd ui && npm install` → CIで成功、ローカル（src-tauriから実行）で失敗
2. `cd ../ui && npm install` → ローカルで成功、CIで失敗
3. `if [ -d ui ]; then ...` → macOS/Linuxで成功、Windowsで失敗（Bash構文）

## Decision

**プロジェクトルートに`package.json`を配置し、npm scriptsでビルドコマンドを定義する。**

### 構成

```
/package.json          # npm scripts定義
/ui/                   # フロントエンド
/src-tauri/
  tauri.conf.json      # npm run tauri:dev/build を呼び出す
```

### package.json

```json
{
  "private": true,
  "scripts": {
    "tauri:dev": "cd ui && npm install && npm run dev",
    "tauri:build": "cd ui && npm install && npm run build"
  }
}
```

### tauri.conf.json

```json
{
  "build": {
    "beforeDevCommand": "npm run tauri:dev",
    "beforeBuildCommand": "npm run tauri:build"
  }
}
```

### 実行方法

```bash
# プロジェクトルートから実行（必須）
cd /path/to/project
cargo tauri dev
cargo tauri build
```

## Consequences

### メリット

- macOS, Windows, Linux全てで同じコマンドが動作
- CIとローカルで同じ手順
- シェル固有構文への依存を排除
- `npm run`内の`cd`はクロスプラットフォームで動作

### デメリット

- プロジェクトルートに`package.json`が必要（既存のRustプロジェクトに追加ファイル）
- `src-tauri`ディレクトリからの直接実行は非サポート

### 制約

- `cargo tauri dev/build`は必ずプロジェクトルートから実行すること
- CI設定で`working-directory`を使用しないこと
