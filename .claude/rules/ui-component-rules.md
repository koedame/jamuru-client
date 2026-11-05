---
paths:
  - "ui/src/**"
---

# UI Component Design Rules

## Core Principle

**Storybook is the single source of truth for UI components.**

All UI components must be designed for both Storybook and Tauri without code duplication.
Differences between environments must be handled through the Adapter pattern.

## Architecture: Pure Component + Adapter Pattern

```
+------------------+     +---------------------+
|  Pure Component  | <-- |  Storybook Stories  |
|  (ChatPanel.tsx) |     |  (.stories.tsx)     |
+------------------+     +---------------------+
         ^
         |
+---------------------+     +----------------+
|  Adapter Component  | <-- |  Tauri App     |
|  (ChatPanelAdapter) |     |  (MainScreen)  |
+---------------------+     +----------------+
```

### Pure Component (Required)

Location: `ui/src/components/{ComponentName}/{ComponentName}.tsx`

Requirements:
- **No Tauri imports** (`@tauri-apps/api/*` is forbidden)
- **No backend state** (use Props for all data)
- **Props-driven** (all inputs via Props, all outputs via callbacks)
- **Stateless or minimal state** (UI state only, e.g., hover, focus)
- **Environment agnostic** (works in any React environment)

Example:
```tsx
// ChatPanel.tsx - Pure component
export interface ChatPanelProps {
  messages: ChatMessageData[];
  onSend?: (message: string) => void;
  disabled?: boolean;
}

export function ChatPanel({ messages, onSend, disabled }: ChatPanelProps) {
  // No Tauri imports, no backend calls
  return (/* JSX */);
}
```

### Adapter Component (For Tauri integration)

Location: `ui/src/components/{ComponentName}/{ComponentName}Adapter.tsx`

Requirements:
- Imports and uses Tauri APIs (`invoke`, `listen`, etc.)
- Manages backend state and polling
- Converts Tauri data formats to Pure component Props
- Wraps Pure component with state management

Example:
```tsx
// ChatPanelAdapter.tsx - Tauri adapter
import { invoke } from "@tauri-apps/api/core";
import { ChatPanel } from "./ChatPanel";

export function ChatPanelAdapter({ connId }: ChatPanelAdapterProps) {
  const [messages, setMessages] = useState<ChatMessageData[]>([]);

  // Poll Tauri backend for messages
  useEffect(() => {
    const poll = async () => {
      const data = await invoke<TauriChatMessage[]>("signaling_get_chat_messages");
      setMessages(convertMessages(data));
    };
    // ...
  }, []);

  return <ChatPanel messages={messages} onSend={handleSend} />;
}
```

## File Structure

```
ui/src/components/
  {ComponentName}/
    {ComponentName}.tsx         # Pure component (Storybook-ready)
    {ComponentName}.stories.tsx # Storybook stories
    {ComponentName}.css         # Component styles
    {ComponentName}.test.tsx    # Unit tests
    {ComponentName}Adapter.tsx  # Tauri adapter (if needed)
    index.ts                    # Exports
```

## Mandatory Rules

### 1. Storybook Coverage

Every Pure component MUST have a corresponding `.stories.tsx` file.

Required story variants:
- `Default` - Standard usage
- `Empty` - Empty/null state
- `Disabled` - Disabled state (if applicable)
- `Error` - Error state (if applicable)
- Edge cases specific to the component

Which states a component needs (interaction states, and Empty/Loading/Partial/Error for data-dependent UI) is defined in [docs-spec/ui/design-guide.md](../../docs-spec/ui/design-guide.md) (「状態のデザイン規則」). UI wording follows [docs-spec/ui/glossary.md](../../docs-spec/ui/glossary.md).

### 2. No Environment Detection

Do not use environment detection in Pure components.

Forbidden:
```tsx
// BAD: Environment detection in Pure component
if (typeof window.__TAURI__ !== 'undefined') {
  // Tauri-specific code
}
```

Correct approach:
```tsx
// GOOD: Use Adapter for environment-specific behavior
// Pure component remains environment-agnostic
```

### 3. Data Format Conversion

Tauri backend data formats must be converted in the Adapter, not in the Pure component.

```tsx
// In Adapter
function convertMessage(msg: TauriChatMessage): ChatMessageData {
  return {
    id: msg.id,
    type: msg.isSystem ? "system" : "other",
    content: msg.content,
    // ...
  };
}
```

### 4. Callback Pattern

Pure components use callback props for actions.
Adapters implement callbacks with Tauri API calls.

```tsx
// Pure component
interface Props {
  onSend?: (message: string) => void;
}

// Adapter implements the callback
const handleSend = async (content: string) => {
  await invoke("signaling_send_chat", { content });
};
```

### 5. Story-First Development

When creating new UI:
1. Design the component API (Props interface)
2. Create the Pure component
3. Write Storybook stories to verify all states
4. Create Adapter for Tauri integration
5. Integrate Adapter into the app

## Verification Checklist

Before merging UI changes:

- [ ] Pure component has no Tauri imports
- [ ] All component states are documented in stories
- [ ] Storybook builds successfully (`npm run storybook:build`)
- [ ] Component works identically in Storybook and Tauri app
- [ ] Props interface is documented with JSDoc comments
- [ ] CSS uses design tokens (variables), not hardcoded values

## Prohibited Patterns

1. **Direct Tauri calls in Pure components**
   ```tsx
   // FORBIDDEN in Pure component
   import { invoke } from "@tauri-apps/api/core";
   ```

2. **Conditional Tauri usage**
   ```tsx
   // FORBIDDEN
   const data = isTauri ? await invoke("...") : mockData;
   ```

3. **Global state in Pure components**
   ```tsx
   // FORBIDDEN - use Props instead
   const [data] = useAtom(globalAtom);
   ```

4. **Deleting or weakening Storybook stories**
   ```tsx
   // FORBIDDEN - stories are the source of truth
   // Do not remove stories to "fix" failing tests
   ```

## Testing

- Pure components: Test with React Testing Library
- Adapters: Test with Tauri test utilities (mock `invoke`)
- Visual regression: Use Storybook snapshot testing
- Whole app (real backend + IPC + webview): GUI E2E in `tests/e2e/tests/gui.rs` (ADR-025).
  Storybook and unit tests never traverse `invoke`, so this is the only layer that
  proves the Adapter wiring works.

### `data-testid` convention (ADR-025)

GUI E2E selects elements by `data-testid`. Class names and visible strings are not
usable as selectors: the former change with styling, the latter with the locale
(the app starts in English on a default machine).

| Rule | Detail |
|------|--------|
| Naming | `kebab-case`, prefixed with the component: `connection-panel`, `connection-panel-create-room` |
| Where | The Pure component's root, plus each control the user can operate |
| Screen state | Expose it as `data-state` on the root (`idle` / `connecting` / `error`) rather than making tests infer it from which elements exist |
| Do not add | When a stable semantic attribute already exists. Settings tabs use `id="tab-<id>"` and `aria-selected`, so they carry no test id |

Add them as scenarios need them. A blanket pass over every component produces
attributes nothing selects on, which then rot.

```tsx
<div className="connection-panel" data-testid="connection-panel" data-state={hasError ? "error" : "idle"}>
  <button data-testid="connection-panel-create-room" onClick={onCreateRoom}>...</button>
</div>
```

**Rebuild `ui/` before running GUI E2E.** The app embeds `ui/dist` at compile time,
so a test id that is only in the source is invisible to the running app.

## デザインの正本は Storybook

UI のデザインは Storybook（`cd ui && npm run storybook`）が正本。デザインファイルは別に持たない。

| 見たいもの | 場所（Storybook の階層） |
|---|---|
| 色・文字・余白・角丸・影・動き・重なり・幅 | `Foundation/Design Tokens`（値の正本は `ui/src/styles/tokens.css`。一覧はその CSS から自動で作る） |
| 部品と状態 | `Components/<グループ>/<部品>`（グループは `Chat` `Mixer` `Room` `Settings` `Terms` `Common`） |
| 画面（実物の画面を偽の backend で描く） | `Screens/<画面>`、ウィンドウに組んだ見た目は `Windows/<ウィンドウ>` |
| 原則・状態の規則 | `Foundation/Introduction` から `docs-spec/ui/design-guide.md` へ |

### ワークフロー

1. 値（色・間隔・タイポグラフィ）は `tokens.css` のトークンを使う。足すときは `tokens.css` に書く（Storybook の一覧にも自動で出る。どのグループにも入らない名前は `designTokens.test.ts` が落とす）
2. Pure Component と `.stories.tsx` を作る（本ファイル冒頭の Pure + Adapter パターンに従う）。`ui/src/components/` と `ui/src/screens/` の部品・画面は、自分の `.stories.tsx` が無いと `storybookCatalog.test.ts` が落ちる（backend に繋ぐだけの層は同テストの一覧に理由を書く）
3. stories の `title` は `Components/<グループ>/<部品>`（3 階層）、画面は `Screens/<画面>`。`docs-spec/ui/design-guide.md`「状態のデザイン規則」の状態を並べる
4. 画面は `ui/src/stories/fakeBackend.tsx` の `withBackend` で backend の答えを与えて、実物の画面を描く。返す値の形は `ui/src/stories/mainFixtures.ts`
5. Storybook を実ブラウザで開き、見た目を目で確かめる
6. Adapter を作り Tauri と結線する

### 食い違ったとき

| 状況 | 対応 |
|------|------|
| 実装が Storybook の見た目から逸脱 | 実装を直す |
| Storybook が仕様（`docs-spec/ui/`）と矛盾 | どちらが正しいかを確認してから、片方を直す。勝手にどちらかへ寄せない |
| 実装する画面・状態が Storybook に無い | 実装と同じ変更で Story を足す。実装だけ先に進めない |

## Related Documents

- [ADR-008: Zero Latency Mode](../../docs-spec/adr/ADR-008-zero-latency-mode.md)
- [docs-spec/ui/design-tokens.md](../../docs-spec/ui/design-tokens.md) - デザイントークン
- [Implementation Quality Rules](./implementation-quality.md)
