# 国際化（i18n）戦略

jamuru の多言語対応の設計と実装方針。

> **関連 ADR**: [ADR-007-i18n-library.md](../adr/ADR-007-i18n-library.md)

---

## 概要

### 対応言語

| 言語 | コード | ステータス | 優先度 |
|------|--------|----------|--------|
| 日本語 | ja | プライマリ | 最高 |
| 英語 | en | プライマリ | 高 |

### 技術スタック

- **ライブラリ**: i18next + react-i18next
- **言語検出**: i18next-browser-languageDetector
- **翻訳ファイル**: JSON形式

---

## ファイル構成

```
ui/
├── locales/
│   ├── ja.json          # 日本語
│   └── en.json          # 英語
├── src/
│   └── i18n/
│       ├── index.ts     # i18n 初期化
│       └── config.ts    # 設定
```

---

## 翻訳キー命名規則

### 形式

```
{namespace}.{section}.{element}
```

### 名前空間

| 名前空間 | 対象 | 例 |
|---------|------|-----|
| common | 共通UI要素 | common.button.cancel |
| session | セッション管理 | session.create.button |
| audio | 音声設定 | audio.device.input |
| mixer | ミキサー | mixer.channel.mute |
| settings | 設定画面 | settings.language.title |
| status | 接続状態 | status.connected |
| preset | プリセット | preset.zeroLatency.name |
| error | エラーメッセージ | error.connection.timeout |
| warning | 警告メッセージ | warning.wifi.detected |
| onboarding | 初回セットアップ | onboarding.welcome.title |

### 命名規則

1. **小文字 + camelCase** を使用
2. **動詞は使わない**（`create` ではなく `creation`）
3. **コンテキストを含める**（`button.cancel` ではなく `common.button.cancel`）

---

## 翻訳ファイル構造

### ja.json（日本語）

```json
{
  "common": {
    "button": {
      "cancel": "キャンセル",
      "ok": "OK",
      "back": "戻る",
      "next": "次へ",
      "save": "保存",
      "retry": "再試行",
      "close": "閉じる"
    },
    "label": {
      "loading": "読み込み中...",
      "required": "必須"
    }
  },
  "session": {
    "create": {
      "button": "ルームを作成",
      "loading": "ルームを作成中..."
    },
    "join": {
      "button": "参加",
      "placeholder": "招待コードを入力",
      "loading": "接続中..."
    },
    "leave": {
      "button": "退出",
      "confirm": "セッションから退出しますか？"
    },
    "invite": {
      "code": "招待コード",
      "url": "招待URL",
      "copied": "コピーしました"
    },
    "participant": {
      "count": "{{count}}名参加中",
      "joined": "{{name}}さんが参加しました",
      "left": "{{name}}さんが退出しました"
    }
  },
  "audio": {
    "device": {
      "input": "入力デバイス",
      "output": "出力デバイス",
      "select": "デバイスを選択",
      "default": "デフォルト"
    },
    "test": {
      "button": "テスト",
      "recording": "録音中...",
      "playing": "再生中..."
    }
  },
  "mixer": {
    "title": "ミキサー",
    "channel": {
      "you": "自分",
      "mute": "ミュート",
      "unmute": "ミュート解除",
      "solo": "ソロ"
    },
    "master": {
      "volume": "マスター音量"
    }
  },
  "settings": {
    "title": "設定",
    "audio": {
      "title": "オーディオ"
    },
    "preset": {
      "title": "モード設定"
    },
    "advanced": {
      "title": "詳細設定",
      "sampleRate": "サンプルレート",
      "frameSize": "フレームサイズ",
      "codec": "コーデック",
      "jitterBuffer": "バッファ",
      "fec": "エラー補正"
    },
    "display": {
      "title": "表示",
      "language": "言語",
      "theme": "テーマ",
      "themeDark": "ダーク",
      "themeLight": "ライト",
      "themeSystem": "システム設定に従う"
    }
  },
  "status": {
    "disconnected": "未接続",
    "connecting": "接続中...",
    "connected": "接続中",
    "unstable": "不安定",
    "error": "切断されました",
    "latency": "{{ms}}ms"
  },
  "preset": {
    "zeroLatency": {
      "name": "最速モード",
      "description": "光回線同士の演奏に最適"
    },
    "ultraLowLatency": {
      "name": "低遅延モード",
      "description": "安定した回線での演奏に"
    },
    "balanced": {
      "name": "バランスモード",
      "description": "通常のインターネット接続に最適"
    },
    "highQuality": {
      "name": "高音質モード",
      "description": "高速回線での録音向け"
    },
    "recommended": "推奨"
  },
  "quality": {
    "excellent": "非常に良好",
    "good": "良好",
    "fair": "普通",
    "poor": "悪い",
    "recommendation": "{{preset}}をお勧めします",
    "switch": "{{preset}}に切り替え"
  },
  "error": {
    "connection": {
      "timeout": {
        "title": "接続できませんでした",
        "message": "相手が見つかりません。招待コードを確認してください。"
      },
      "refused": {
        "title": "接続が拒否されました",
        "message": "ルームが存在しないか、終了しています。"
      },
      "lost": {
        "title": "接続が切れました",
        "message": "ネットワークを確認してください。"
      }
    },
    "audio": {
      "device": {
        "title": "マイクが使えません",
        "message": "他のアプリがマイクを使用中かもしれません。"
      },
      "permission": {
        "title": "マイクの許可が必要です",
        "message": "設定からマイクへのアクセスを許可してください。"
      }
    },
    "room": {
      "full": {
        "title": "ルームが満員です",
        "message": "参加者数が上限に達しています。"
      },
      "password": {
        "title": "パスワードが正しくありません",
        "message": "入力したパスワードを確認してください。"
      },
      "notFound": {
        "title": "ルームが見つかりません",
        "message": "招待コードを確認してください。"
      }
    },
    "generic": {
      "title": "エラーが発生しました",
      "message": "しばらく待ってから再試行してください。"
    },
    "action": {
      "retry": "再試行",
      "changeDevice": "デバイスを変更",
      "openSettings": "設定を開く"
    }
  },
  "warning": {
    "wifi": {
      "title": "WiFi接続を検出しました",
      "message": "有線接続を推奨します。WiFiは遅延が不安定になる可能性があります。"
    },
    "speaker": {
      "title": "スピーカー使用を検出しました",
      "message": "エコー防止のためヘッドフォンを推奨します。"
    },
    "packetLoss": {
      "title": "パケットロス率が高くなっています",
      "message": "ネットワーク状態を確認してください。"
    },
    "bandwidth": {
      "title": "帯域不足のため音質を下げました",
      "message": "ネットワーク状態が回復すると自動的に戻ります。"
    }
  },
  "onboarding": {
    "welcome": {
      "title": "ようこそ",
      "subtitle": "音楽仲間とオンラインで一緒に演奏しよう",
      "requirements": "ヘッドフォンと有線接続を推奨します"
    },
    "mic": {
      "title": "マイクを選択",
      "subtitle": "使用するマイクを選んでください",
      "test": "声を出してメーターが動くことを確認"
    },
    "speaker": {
      "title": "スピーカーを選択",
      "subtitle": "使用するスピーカーを選んでください",
      "testButton": "テスト音を再生"
    },
    "test": {
      "title": "音声テスト",
      "subtitle": "マイクとスピーカーをテストします",
      "recordButton": "録音して確認",
      "question": "自分の声が聞こえましたか？"
    },
    "preset": {
      "title": "モードを選択",
      "subtitle": "まずはバランスモードがおすすめです"
    },
    "complete": {
      "title": "準備完了！",
      "subtitle": "セッションを始めましょう"
    },
    "skip": "後で設定する"
  },
  "notification": {
    "settingsChanged": "設定を変更しました",
    "connected": "接続しました",
    "disconnected": "切断しました",
    "bandwidthRecovered": "帯域が回復しました"
  }
}
```

### en.json（英語）

```json
{
  "common": {
    "button": {
      "cancel": "Cancel",
      "ok": "OK",
      "back": "Back",
      "next": "Next",
      "save": "Save",
      "retry": "Retry",
      "close": "Close"
    },
    "label": {
      "loading": "Loading...",
      "required": "Required"
    }
  },
  "session": {
    "create": {
      "button": "Create Room",
      "loading": "Creating room..."
    },
    "join": {
      "button": "Join",
      "placeholder": "Enter invite code",
      "loading": "Connecting..."
    },
    "leave": {
      "button": "Leave",
      "confirm": "Leave this session?"
    },
    "invite": {
      "code": "Invite Code",
      "url": "Invite URL",
      "copied": "Copied"
    },
    "participant": {
      "count": "{{count}} participants",
      "joined": "{{name}} joined",
      "left": "{{name}} left"
    }
  },
  "status": {
    "disconnected": "Disconnected",
    "connecting": "Connecting...",
    "connected": "Connected",
    "unstable": "Unstable",
    "error": "Disconnected",
    "latency": "{{ms}}ms"
  },
  "preset": {
    "zeroLatency": {
      "name": "Fastest Mode",
      "description": "For fiber-to-fiber connections"
    },
    "ultraLowLatency": {
      "name": "Low Delay Mode",
      "description": "For stable connections"
    },
    "balanced": {
      "name": "Balanced Mode",
      "description": "For typical internet connections"
    },
    "highQuality": {
      "name": "High Quality Mode",
      "description": "For high-speed recording"
    },
    "recommended": "Recommended"
  }
}
```

---

## 補間（Interpolation）

### 基本

```json
{
  "participant": {
    "count": "{{count}}名参加中"
  }
}
```

```typescript
t('session.participant.count', { count: 3 })
// → "3名参加中"
```

### 複数形（Pluralization）

i18next の pluralization を使用：

```json
{
  "participant": {
    "count_one": "{{count}} participant",
    "count_other": "{{count}} participants"
  }
}
```

```typescript
t('session.participant.count', { count: 1 })
// → "1 participant"

t('session.participant.count', { count: 3 })
// → "3 participants"
```

---

## フォールバック

### フォールバック順序

1. 選択言語で検索（例: `ja`）
2. 見つからない場合、フォールバック言語で検索（`en`）
3. 見つからない場合、キー名を表示

### 設定

```typescript
// src/i18n/config.ts
export const i18nConfig = {
  fallbackLng: 'en',
  supportedLngs: ['ja', 'en'],
  defaultNS: 'translation',
  interpolation: {
    escapeValue: false, // React はデフォルトでエスケープ
  },
  detection: {
    order: ['localStorage', 'navigator'],
    caches: ['localStorage'],
  },
};
```

---

## 言語検出と切り替え

### 自動検出

```typescript
// src/i18n/index.ts
import i18n from 'i18next';
import LanguageDetector from 'i18next-browser-languagedetector';

i18n
  .use(LanguageDetector)
  .init({
    detection: {
      order: ['localStorage', 'navigator'],
      caches: ['localStorage'],
    },
  });
```

### 手動切り替え

```typescript
// 言語を変更
i18n.changeLanguage('en');

// 現在の言語を取得
const currentLang = i18n.language;
```

### React コンポーネントでの使用

```tsx
import { useTranslation } from 'react-i18next';

function LanguageSelector() {
  const { i18n } = useTranslation();

  const handleChange = (lang: string) => {
    i18n.changeLanguage(lang);
  };

  return (
    <select
      value={i18n.language}
      onChange={(e) => handleChange(e.target.value)}
    >
      <option value="ja">日本語</option>
      <option value="en">English</option>
    </select>
  );
}
```

---

## 永続化

### localStorage への保存

```typescript
// 言語変更時に自動保存（LanguageDetector が処理）
i18n.changeLanguage('ja');
// → localStorage に 'i18nextLng': 'ja' が保存される
```

### 設定画面との連携

```typescript
function saveLanguageSetting(lang: string) {
  i18n.changeLanguage(lang);
  // Tauri の設定保存と同期
  invoke('save_settings', { language: lang });
}
```

---

## 翻訳ワークフロー

### 新しいキーを追加する場合

1. `en.json` に英語キーを追加
2. `ja.json` に日本語翻訳を追加
3. コンポーネントで `t('namespace.key')` を使用
4. CI で未翻訳キーをチェック

### 翻訳の検証

```typescript
// scripts/check-i18n.ts
import ja from '../ui/locales/ja.json';
import en from '../ui/locales/en.json';

function getAllKeys(obj: object, prefix = ''): string[] {
  return Object.entries(obj).flatMap(([key, value]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'object') {
      return getAllKeys(value, path);
    }
    return path;
  });
}

const jaKeys = new Set(getAllKeys(ja));
const enKeys = new Set(getAllKeys(en));

// 英語にあって日本語にないキー
const missingInJa = [...enKeys].filter(k => !jaKeys.has(k));
if (missingInJa.length > 0) {
  console.error('Missing in ja.json:', missingInJa);
  process.exit(1);
}
```

---

## アクセシビリティ考慮

### 言語属性

```tsx
// App.tsx
function App() {
  const { i18n } = useTranslation();

  return (
    <html lang={i18n.language}>
      {/* ... */}
    </html>
  );
}
```

### RTL 対応（将来）

現時点では RTL 言語（アラビア語、ヘブライ語など）は対象外。
将来的に対応する場合は `dir="rtl"` 属性とスタイル調整が必要。

---

## 実装チェックリスト

### 初期セットアップ

- [ ] i18next, react-i18next, i18next-browser-languagedetector をインストール
- [ ] `ui/src/i18n/index.ts` で初期化
- [ ] `ui/locales/ja.json`, `ui/locales/en.json` を作成
- [ ] App.tsx で `I18nextProvider` をラップ

### 開発時

- [ ] 新しい文字列は必ず翻訳キーを使用
- [ ] ハードコードされた日本語/英語を禁止
- [ ] 補間は `{{variable}}` 形式を使用

### CI

- [ ] 未翻訳キーの検出スクリプトを実行
- [ ] 翻訳ファイルの JSON バリデーション
