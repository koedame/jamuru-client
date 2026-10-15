import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent } from "@testing-library/react";
import {
  findPresenceElement,
  interactiveOf,
  MAX_SENDS_PER_SECOND,
  presenceIdOf,
  targetOf,
  watchPresence,
} from "./presence";
import type { Presence } from "./tauri";

function mount(html: string): HTMLElement {
  document.body.innerHTML = html;
  return document.body;
}

/** jsdom has no layout: a part is given the box it would have. */
function box(el: Element, left: number, top: number, width: number, height: number) {
  el.getBoundingClientRect = () =>
    ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) }) as DOMRect;
}

describe("interactiveOf", () => {
  beforeEach(() => document.body.replaceChildren());

  // Verifies: REQ-RMT-034
  it("ボタンの中の要素を指したとき、そのボタンが部品として返ること", () => {
    mount('<button id="b"><span id="label">Mute</span></button>');

    expect(interactiveOf(document.getElementById("label"))).toBe(document.getElementById("b"));
  });

  // Verifies: REQ-RMT-034
  it("押せもフォーカスもできない要素を指したとき、部品は無いこと", () => {
    mount('<div id="plain"><p id="text">hello</p></div>');

    expect(interactiveOf(document.getElementById("text"))).toBeNull();
  });

  // Verifies: REQ-RMT-034
  it("使えないボタンや、フォーカスを受けない要素を指したとき、部品は無いこと", () => {
    mount('<button id="off" disabled>x</button><div id="skip" tabindex="-1">y</div><div id="aria" role="button" aria-disabled="true">z</div>');

    expect(interactiveOf(document.getElementById("off"))).toBeNull();
    expect(interactiveOf(document.getElementById("skip"))).toBeNull();
    expect(interactiveOf(document.getElementById("aria"))).toBeNull();
  });

  // Verifies: REQ-RMT-034
  it("リンク・入力欄・role で押せるとされた要素・data-presence-id を付けた要素は、部品になること", () => {
    mount(
      '<a id="a" href="/x">a</a><input id="i" /><div id="r" role="switch"></div><div id="p" data-presence-id="fader"></div><input id="h" type="hidden" />'
    );

    for (const id of ["a", "i", "r", "p"]) {
      expect(interactiveOf(document.getElementById(id))).toBe(document.getElementById(id));
    }
    expect(interactiveOf(document.getElementById("h"))).toBeNull();
  });
});

describe("presenceIdOf", () => {
  beforeEach(() => document.body.replaceChildren());

  // Verifies: REQ-RMT-034
  it("data-presence-id と data-testid があるとき、data-presence-id が先に使われること", () => {
    mount('<button id="b" data-presence-id="mute" data-testid="mixer-mute">Mute</button><button id="c" data-testid="leave-room">x</button>');

    expect(presenceIdOf(document.getElementById("b")!)).toBe("mute");
    expect(presenceIdOf(document.getElementById("c")!)).toBe("leave-room");
  });

  // Verifies: REQ-RMT-034
  it("名前の印が無い部品のとき、文字ではなくタグの並びから名前が決まり、言語が違っても同じになること", () => {
    const html = (label: string) =>
      `<section data-testid="mixer"><button>${label}</button><button>Other</button></section>`;
    mount(html("ミュート"));
    const japanese = presenceIdOf(document.querySelector("button")!);
    mount(html("Mute"));
    const english = presenceIdOf(document.querySelector("button")!);

    expect(japanese).toBe('button:nth-of-type(1)'.replace(/^/, 'section[data-testid="mixer"] > '));
    expect(english).toBe(japanese);
  });

  // Verifies: REQ-RMT-034
  it("描くたびに変わる id を持つ部品のとき、その id は名前に使われないこと", () => {
    mount('<div data-testid="panel"><input id=":r3:" /></div>');

    expect(presenceIdOf(document.querySelector("input")!)).toBe('div[data-testid="panel"] > input');
  });

  // Verifies: REQ-RMT-034
  it("名前が長すぎるとき、部品は名指しできないものとして扱われること", () => {
    mount(`<button data-presence-id="${"a".repeat(201)}">x</button>`);

    expect(presenceIdOf(document.querySelector("button")!)).toBeNull();
  });

  // Verifies: REQ-RMT-034
  it("名前から部品を探すとき、同じ名前の部品が見つかり、無い名前では見つからないこと", () => {
    mount('<button data-testid="a">a</button><button data-testid="b">b</button>');

    expect(findPresenceElement("b")).toBe(document.querySelector('[data-testid="b"]'));
    expect(findPresenceElement("c")).toBeNull();
  });
});

describe("targetOf", () => {
  // Verifies: REQ-RMT-034
  it("部品の中のどこを指したかが、0 から 1 の割合で返ること", () => {
    mount('<button data-testid="b">x</button>');
    const button = document.querySelector("button")!;
    box(button, 100, 200, 80, 40);

    expect(targetOf(button, 120, 230)).toEqual({ id: "b", x: 0.25, y: 0.75 });
    // Outside the part (a pointer on its border) is held to the part.
    expect(targetOf(button, 500, 100)).toEqual({ id: "b", x: 1, y: 0 });
  });
});

describe("watchPresence", () => {
  let sent: Presence[];
  let stop: () => void;
  let refresh: () => void;

  function start(extra?: () => Partial<Presence>) {
    const watch = watchPresence((presence) => sent.push(presence), extra);
    stop = watch.stop;
    refresh = watch.refresh;
  }

  function point(el: Element, clientX: number, clientY: number) {
    fireEvent.pointerMove(el, { clientX, clientY });
  }

  beforeEach(() => {
    vi.useFakeTimers();
    sent = [];
    mount('<button data-testid="mute">Mute</button><p id="text">text</p><input data-testid="name" />');
    box(document.querySelector("button")!, 0, 0, 100, 100);
  });

  afterEach(() => {
    stop();
    vi.useRealTimers();
  });

  // Verifies: REQ-RMT-034
  it("ボタンの上でポインタが動いたとき、部品の名前とその中の位置が送られること", () => {
    start();

    point(document.querySelector("button")!, 50, 25);

    expect(sent.slice(-1)[0]).toEqual({ hover: { id: "mute", x: 0.5, y: 0.25 }, focus: null });
  });

  // Verifies: REQ-RMT-034
  it("部品の外でポインタが動いたとき、何も送られないこと（動きの軌跡は送らない）", () => {
    start();
    refresh();
    sent = [];

    for (let i = 0; i < 20; i++) {
      point(document.getElementById("text")!, i, i);
      act(() => vi.advanceTimersByTime(150));
    }

    expect(sent).toEqual([]);
  });

  // Verifies: REQ-RMT-034
  it("部品の上から外へ出たとき、指している部品が無くなったことが 1 度だけ送られること", () => {
    start();
    point(document.querySelector("button")!, 10, 10);
    act(() => vi.advanceTimersByTime(150));
    sent = [];

    point(document.getElementById("text")!, 1, 1);
    point(document.getElementById("text")!, 2, 2);

    expect(sent).toEqual([{ hover: null, focus: null }]);
  });

  // Verifies: REQ-RMT-034
  it("ポインタが部品の上で動き続けたとき、送信は 1 秒に 10 回までで、最後の位置は落とさないこと", () => {
    start();
    const button = document.querySelector("button")!;

    for (let ms = 0; ms < 1000; ms += 5) {
      point(button, (ms / 5) % 100, 10);
      act(() => vi.advanceTimersByTime(5));
    }
    act(() => vi.advanceTimersByTime(200));

    expect(sent.length).toBeLessThanOrEqual(MAX_SENDS_PER_SECOND + 2);
    expect(sent.length).toBeGreaterThanOrEqual(MAX_SENDS_PER_SECOND - 1);
    // 995 ms is the last move, at x = 199 % 100 = 99.
    expect(sent.slice(-1)[0]?.hover).toEqual({ id: "mute", x: 0.99, y: 0.1 });
  });

  // Verifies: REQ-RMT-034
  it("入力欄にフォーカスが移ったとき、その部品が focus として送られ、外れたら無くなること", () => {
    start();
    const input = document.querySelector("input")!;
    box(input, 0, 0, 200, 20);

    act(() => input.focus());
    act(() => vi.advanceTimersByTime(150));
    expect(sent.slice(-1)[0]?.focus).toEqual({ id: "name", x: 0.5, y: 0.5 });

    act(() => input.blur());
    act(() => vi.advanceTimersByTime(150));
    expect(sent.slice(-1)[0]?.focus).toBeNull();
  });

  // Verifies: REQ-RMT-034
  it("手伝う人の設定パネルが開いたとき、動きが無くても panel つきの状態が送られること", () => {
    let panel = false;
    start(() => ({ panel }));
    refresh();
    expect(sent.slice(-1)[0]).toEqual({ hover: null, focus: null, panel: false });

    panel = true;
    act(() => vi.advanceTimersByTime(150));
    refresh();

    expect(sent.slice(-1)[0]).toEqual({ hover: null, focus: null, panel: true });
  });

  // Verifies: REQ-RMT-034
  it("止めたあとにポインタが動いたとき、何も送られないこと", () => {
    start();
    refresh();
    stop();
    sent = [];

    point(document.querySelector("button")!, 10, 10);
    act(() => vi.advanceTimersByTime(500));

    expect(sent).toEqual([]);
  });
});
