import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';

import { TermsText, parseTerms } from './TermsText';
// The file the app bundles (`src-tauri/src/terms.rs`), read from the repository.
import terms from '../../../../docs/terms.md?raw';

describe('the terms text', () => {
  it('a heading, a paragraph and a nested list are given, each is drawn as what it is', () => {
    const blocks = parseTerms('# 題\n\n## 第 1 条\n\n本文です。\n\n1. 一つ目\n   1. 入れ子\n- 箇条');

    expect(blocks).toEqual([
      { kind: 'h1', text: '題' },
      { kind: 'h2', text: '第 1 条' },
      { kind: 'p', text: '本文です。' },
      { kind: 'item', text: '1. 一つ目', depth: 0 },
      { kind: 'item', text: '1. 入れ子', depth: 1 },
      { kind: 'item', text: '- 箇条', depth: 0 },
    ]);
  });

  it('a link is given, only its label is shown and nothing can navigate the window away', () => {
    const { container } = render(
      <TermsText text="詳しくは [LICENSE](https://example.com/LICENSE) へ。**大事**です。" />
    );

    expect(screen.getByText('LICENSE')).toBeInTheDocument();
    expect(container.querySelector('a')).toBeNull();
    expect(screen.getByText('大事').tagName).toBe('STRONG');
  });

  it('the license is shown as plain text, its line breaks are kept', () => {
    const { container } = render(<TermsText text={'1. DEFINITIONS\n\n"Software" means'} format="plain" />);

    expect(container.querySelector('pre')?.textContent).toBe('1. DEFINITIONS\n\n"Software" means');
  });

  // Verifies: REQ-TRM-005
  it('the published terms are drawn, all 15 articles are there and no link can be followed', () => {
    const { container } = render(<TermsText text={terms} />);

    for (let n = 1; n <= 15; n++) {
      expect(screen.getByText(new RegExp(`^第 ${n} 条（`))).toBeInTheDocument();
    }
    expect(container.querySelector('a')).toBeNull();
    expect(container.textContent).not.toContain('**');
  });
});
