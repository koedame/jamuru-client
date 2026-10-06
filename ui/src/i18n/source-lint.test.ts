/**
 * Source lint for the i18n requirement: every string the UI shows comes from
 * the locale bundles, so switching language never leaves a mix of languages.
 *
 * The two ways a string escapes the bundles:
 *  - Japanese written straight into a component (as a prop default, a JSX
 *    literal or a `t()` default value).
 *  - English written straight into JSX (as text or as an `aria-label`,
 *    `title`, `placeholder` or `alt`). English is the fallback language, so
 *    it looks right in English and stays English when the UI is Japanese.
 *  - A `t("some.key")` whose key is missing from a bundle. i18next then falls
 *    back to English (or, with a default value, to that default), so the
 *    screen silently shows the wrong language.
 */

import { describe, it, expect } from 'vitest';
import ts from 'typescript';

import ja from '../../locales/ja.json';
import en from '../../locales/en.json';

const rawSources = import.meta.glob('../**/*.{ts,tsx}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** Files that ship in the app. Stories, tests and mocks are excluded. */
const runtimeSources = Object.entries(rawSources).filter(
  ([path]) => !/\.(stories|test)\.tsx?$/.test(path)
);

const JAPANESE = /[぀-ヿ㐀-鿿]/;

/**
 * Japanese that is allowed in runtime code, with the reason.
 * A language's own name is shown in that language in every UI language, so
 * it is not translated.
 */
const ALLOWED_JAPANESE: { file: string; text: string; reason: string }[] = [
  {
    file: '../components/SettingsPanel/tabs/GeneralTab.tsx',
    text: '"日本語"',
    reason: 'language selector shows each language by its own name',
  },
  {
    file: '../Catalog.tsx',
    text: '日本語',
    reason: 'language toggle shows each language by its own name',
  },
  {
    file: '../components/ChatPanel/ChatMessage.tsx',
    text: '参加|入室|join',
    reason: 'keywords that classify text, not text shown to the user',
  },
  {
    file: '../components/ChatPanel/ChatMessage.tsx',
    text: '退出|退室|left|leave',
    reason: 'keywords that classify text, not text shown to the user',
  },
];

/**
 * English that is allowed in JSX, with the reason. An entry without `text`
 * allows the whole file.
 */
const ALLOWED_ENGLISH: { file: string; text?: string; reason: string }[] = [
  {
    file: '../Catalog.tsx',
    reason: 'development catalog, only rendered with VITE_CATALOG_MODE',
  },
  {
    file: '../screens/MainScreen.tsx',
    text: 'jamuru',
    reason: 'the product name is not translated',
  },
];

/** Attributes whose value a user or a screen reader reads. */
const USER_FACING_ATTRIBUTES = new Set([
  'aria-label',
  'aria-description',
  'aria-roledescription',
  'aria-placeholder',
  'title',
  'placeholder',
  'alt',
]);

/** Two letters in a row: a word, not a symbol, a channel letter or an emoji. */
const ENGLISH_WORD = /[A-Za-z]{2,}/;

/** Units read the same in every language. */
const UNITS = /\b(ms|dB|kHz|Hz|ch)\b/g;

/**
 * The string pieces an expression can put on screen. Calls are not followed:
 * `t("key")` is the way to translate, so it is never a literal.
 */
function literalTexts(node: ts.Expression): string[] {
  if (ts.isStringLiteralLike(node)) return [node.text];
  if (ts.isTemplateExpression(node)) {
    return [node.head.text, ...node.templateSpans.map((span) => span.literal.text)];
  }
  if (ts.isParenthesizedExpression(node)) return literalTexts(node.expression);
  if (ts.isConditionalExpression(node)) {
    return [...literalTexts(node.whenTrue), ...literalTexts(node.whenFalse)];
  }
  if (ts.isBinaryExpression(node)) {
    const op = node.operatorToken.kind;
    if (op === ts.SyntaxKind.AmpersandAmpersandToken) return literalTexts(node.right);
    if (
      op === ts.SyntaxKind.QuestionQuestionToken ||
      op === ts.SyntaxKind.BarBarToken ||
      op === ts.SyntaxKind.PlusToken
    ) {
      return [...literalTexts(node.left), ...literalTexts(node.right)];
    }
  }
  return [];
}

/**
 * English words written straight into JSX: as text, as `{"text"}`, or as a
 * user-facing attribute. Strings a function returns for display are not
 * found (a `switch` returning "Balanced" looks like any other string), so
 * those still rely on review.
 */
function findEnglishLiterals(source: string): { line: number; text: string }[] {
  const sourceFile = ts.createSourceFile(
    'component.tsx',
    source,
    ts.ScriptTarget.ES2020,
    true,
    ts.ScriptKind.TSX
  );
  const found: { line: number; text: string }[] = [];
  const report = (node: ts.Node, text: string) => {
    const shown = text.replace(/&#?\w+;/g, ' ').replace(UNITS, ' ');
    if (!ENGLISH_WORD.test(shown)) return;
    const line = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1;
    found.push({ line, text: text.trim() });
  };
  const visit = (node: ts.Node) => {
    if (ts.isJsxText(node)) {
      report(node, node.text);
    } else if (
      ts.isJsxAttribute(node) &&
      USER_FACING_ATTRIBUTES.has(node.name.getText(sourceFile)) &&
      node.initializer
    ) {
      const init = node.initializer;
      const texts = ts.isJsxExpression(init)
        ? init.expression
          ? literalTexts(init.expression)
          : []
        : ts.isStringLiteral(init)
          ? [init.text]
          : [];
      texts.forEach((text) => report(init, text));
    } else if (ts.isJsxExpression(node) && !ts.isJsxAttribute(node.parent) && node.expression) {
      literalTexts(node.expression).forEach((text) => report(node, text));
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return found;
}

/** Blank out comments, keeping line numbers, so Japanese notes do not count. */
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/(^|[^:"'`\\])\/\/.*$/gm, '$1');
}

type Bundle = Record<string, unknown>;

function flatten(bundle: Bundle, prefix = ''): string[] {
  return Object.entries(bundle).flatMap(([key, value]) =>
    value !== null && typeof value === 'object'
      ? flatten(value as Bundle, `${prefix}${key}.`)
      : [`${prefix}${key}`]
  );
}

/** `count_one` / `count_other` are the plural forms of `count`. */
function withoutPluralSuffix(key: string): string {
  return key.replace(/_(zero|one|two|few|many|other)$/, '');
}

const enKeys = new Set(flatten(en).map(withoutPluralSuffix));
const jaKeys = new Set(flatten(ja).map(withoutPluralSuffix));

/**
 * String-literal keys in the first argument of every `t(...)` call. The
 * argument is scanned rather than matched, so `t(cond ? "a.b" : "c.d")` yields
 * both keys. Template literals are dynamic and skipped.
 */
function extractTranslationKeys(source: string): string[] {
  const keys: string[] = [];
  for (const match of source.matchAll(/\bt\(/g)) {
    let depth = 0;
    let i = (match.index ?? 0) + match[0].length;
    for (; i < source.length; i++) {
      const c = source[i];
      if (c === '"' || c === "'" || c === '`') {
        const start = i + 1;
        for (i = start; i < source.length && source[i] !== c; i++) {
          if (source[i] === '\\') i++;
        }
        const literal = source.slice(start, i);
        if (c !== '`' && /^[A-Za-z][A-Za-z0-9]*(\.[A-Za-z0-9_]+)+$/.test(literal)) {
          keys.push(literal);
        }
      } else if (c === '(') {
        depth++;
      } else if (c === ')') {
        if (depth === 0) break;
        depth--;
      } else if (c === ',' && depth === 0) {
        break;
      }
    }
  }
  return keys;
}

describe('i18n source lint', () => {
  // Given a component under ui/src
  // When its source is read
  // Then no Japanese text is written in it outside comments
  //
  // Verifies: REQ-I18N-105
  it('has no hard-coded Japanese in runtime source', () => {
    const found: string[] = [];
    for (const [file, source] of runtimeSources) {
      stripComments(source)
        .split('\n')
        .forEach((line, index) => {
          if (!JAPANESE.test(line)) return;
          const allowed = ALLOWED_JAPANESE.some(
            (a) => a.file === file && line.includes(a.text)
          );
          if (!allowed) found.push(`${file}:${index + 1}: ${line.trim()}`);
        });
    }
    expect(found).toEqual([]);
  });

  // Given a component under ui/src
  // When its JSX is read
  // Then no English text or user-facing attribute is written in it
  //
  // Verifies: REQ-I18N-105
  it('has no hard-coded English in JSX', () => {
    const found: string[] = [];
    for (const [file, source] of runtimeSources) {
      const rules = ALLOWED_ENGLISH.filter((a) => a.file === file);
      if (rules.some((a) => a.text === undefined)) continue;
      for (const { line, text } of findEnglishLiterals(source)) {
        if (rules.some((a) => a.text === text)) continue;
        found.push(`${file}:${line}: ${text}`);
      }
    }
    expect(found).toEqual([]);
  });

  // Given a component calls t("some.key")
  // When the key is missing from en.json or ja.json
  // Then this fails, instead of the screen falling back to another language
  //
  // Verifies: REQ-I18N-105
  it('has every statically referenced translation key in both bundles', () => {
    const missing: string[] = [];
    for (const [file, source] of runtimeSources) {
      for (const key of extractTranslationKeys(stripComments(source))) {
        if (!enKeys.has(key)) missing.push(`${file}: "${key}" is not in en.json`);
        if (!jaKeys.has(key)) missing.push(`${file}: "${key}" is not in ja.json`);
      }
    }
    expect(missing).toEqual([]);
  });

  // Given a key exists in one bundle
  // When the other bundle lacks it
  // Then this fails, since that language would show English for the key
  //
  // Verifies: REQ-I18N-105
  it('has the same keys in en.json and ja.json', () => {
    const onlyInEn = [...enKeys].filter((k) => !jaKeys.has(k));
    const onlyInJa = [...jaKeys].filter((k) => !enKeys.has(k));
    expect({ onlyInEn, onlyInJa }).toEqual({ onlyInEn: [], onlyInJa: [] });
  });

  describe('extractTranslationKeys', () => {
    it('reads both branches of a conditional key', () => {
      expect(
        extractTranslationKeys('t(muted ? "mixer.channel.unmute" : "mixer.channel.mute")')
      ).toEqual(['mixer.channel.unmute', 'mixer.channel.mute']);
    });

    it('ignores the default value and interpolation options', () => {
      expect(
        extractTranslationKeys('t("common.none", "None") + t("a.b", { count: 1 })')
      ).toEqual(['common.none', 'a.b']);
    });

    it('skips dynamic keys', () => {
      expect(extractTranslationKeys('t(`chat.emoji.${category}`)')).toEqual([]);
      expect(extractTranslationKeys('t(key)')).toEqual([]);
    });
  });

  describe('findEnglishLiterals', () => {
    const texts = (source: string) => findEnglishLiterals(source).map((f) => f.text);

    it('finds English JSX text', () => {
      expect(texts('const a = <span>Waiting for connection...</span>;')).toEqual([
        'Waiting for connection...',
      ]);
    });

    it('finds English in user-facing attributes', () => {
      expect(
        texts('const a = <button aria-label="Close" title="Close" placeholder="User" alt="Logo" />;')
      ).toEqual(['Close', 'Close', 'User', 'Logo']);
    });

    it('finds English in a template literal, a conditional and a fallback', () => {
      expect(texts('const a = <b aria-label={`${count} days ago`} />;')).toEqual(['days ago']);
      expect(texts('const a = <b title={on ? "Mute" : "Unmute"} />;')).toEqual(['Mute', 'Unmute']);
      expect(texts('const a = <b title={label ?? "Settings"} />;')).toEqual(['Settings']);
      expect(texts('const a = <b>{"Loading..."}</b>;')).toEqual(['Loading...']);
    });

    it('ignores translated text, symbols and non-visible attributes', () => {
      expect(
        texts(
          'const a = <b aria-label={t("common.button.close")} className="quick-reactions" value="Total" data-testid="x">{t("a.b")}{count} + L &#9662;&nbsp;</b>;'
        )
      ).toEqual([]);
    });

    it('ignores units', () => {
      expect(texts('const a = <b>{x} ms {y} kHz {z} dB</b>;')).toEqual([]);
    });

    it('ignores English outside JSX', () => {
      expect(texts('const label = "Balanced"; foo("Close");')).toEqual([]);
    });
  });
});
