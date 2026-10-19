/**
 * Splits chat text into plain text and web addresses so the addresses can be
 * shown as links. Only `http(s)://` addresses are recognised: nothing that
 * could start another program is ever turned into a link.
 */

export type ChatTextPart =
  | { kind: "text"; text: string }
  | { kind: "link"; text: string; url: string };

// An address runs over the characters a URL may contain, so it stops at
// whitespace and at Japanese text such as "https://example.com を見て".
const URL_PATTERN = /https?:\/\/[A-Za-z0-9\-._~:/?#[\]@!$&'()*+,;=%]+/gi;

// Punctuation that ends a sentence rather than the address.
const TRAILING = /[.,;:!?'*]+$/;

const CLOSERS: Record<string, string> = { ")": "(", "]": "[" };

/** Drops what follows the address in the sentence: trailing punctuation and an unmatched ")" or "]". */
function trimAddress(raw: string): string {
  let url = raw;
  for (;;) {
    const before = url;
    url = url.replace(TRAILING, "");
    const last = url.slice(-1);
    const opener = CLOSERS[last];
    if (opener && count(url, last) > count(url, opener)) url = url.slice(0, -1);
    if (url === before) return url;
  }
}

function count(text: string, char: string): number {
  return text.split(char).length - 1;
}

/** A address with a host after the scheme (so "https://" alone stays text). */
function hasHost(url: string): boolean {
  return /^https?:\/\/[^/?#:@]/i.test(url);
}

export function splitLinks(text: string): ChatTextPart[] {
  const parts: ChatTextPart[] = [];
  let last = 0;
  for (const match of text.matchAll(URL_PATTERN)) {
    const url = trimAddress(match[0]);
    if (!hasHost(url)) continue;
    const start = match.index ?? 0;
    if (start > last) parts.push({ kind: "text", text: text.slice(last, start) });
    parts.push({ kind: "link", text: url, url });
    last = start + url.length;
  }
  if (last < text.length) parts.push({ kind: "text", text: text.slice(last) });
  return parts;
}
