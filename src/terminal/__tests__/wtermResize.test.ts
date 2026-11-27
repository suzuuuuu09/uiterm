import { WTerm } from "@wterm/dom";
import { expect, it, vi } from "vite-plus/test";

it.each([true, false])(
  "preserves the viewport's previous follow state (%s) on resize",
  (pinned) => {
    const terminal = {
      element: {
        ownerDocument: {},
        scrollHeight: 1134,
        scrollTop: pinned ? 634 : 300,
        clientHeight: 250,
      },
      bridge: { resize: vi.fn(), getCols: () => 57, getRows: () => 14 },
      renderer: { beforeMutation() {} },
      _pinnedToBottom: pinned,
      _shouldScrollToBottom: pinned,
      _pendingResizeScrollTop: null,
      _textCapture: { cancel() {} },
      _rectangleDrag: { cancel() {} },
      _historySelection: { invalidate() {} },
      _outputAnnouncements: { invalidate() {} },
      _isScrolledToBottom() {
        const { scrollHeight, scrollTop, clientHeight } = this.element;
        return scrollHeight - scrollTop - clientHeight < 5;
      },
      _updateSynchronizedOutput: () => false,
      _setupRenderer() {},
      _scheduleRender() {},
      _invalidateSearch() {},
    };

    WTerm.prototype.resize.call(terminal as unknown as WTerm, 57, 14);

    expect(terminal._shouldScrollToBottom).toBe(pinned);
  },
);

it("requests the measured size even when it matches the applied grid", () => {
  const terminal = {
    cols: 132,
    rows: 43,
    bridge: {},
    _measureGridSize: () => ({ cols: 132, rows: 43 }),
    onResizeRequest: vi.fn(),
    resize: vi.fn(),
  };

  WTerm.prototype.fit.call(terminal as unknown as WTerm);

  expect(terminal.onResizeRequest).toHaveBeenCalledWith(132, 43);
  expect(terminal.resize).not.toHaveBeenCalled();
});

it("keeps following output when a programmatic scroll event arrives after the viewport shrinks", () => {
  const ownerDocument = Object.assign(new EventTarget(), {
    createElement: () => ({ setAttribute() {} }),
    visibilityState: "hidden",
  });
  const host = Object.assign(new EventTarget(), {
    ownerDocument,
    appendChild() {},
    classList: { add() {}, toggle() {} },
    scrollTop: 1438,
    scrollHeight: 1848,
    clientHeight: 350,
  });
  vi.stubGlobal("document", ownerDocument);

  try {
    const terminal = new WTerm(host as unknown as HTMLElement) as unknown as {
      _programmaticScrollTop: number | null;
      _pinnedToBottom: boolean;
    };
    terminal._programmaticScrollTop = 1438;

    host.dispatchEvent(new Event("scroll"));
    expect(terminal._pinnedToBottom).toBe(true);
    expect(terminal._programmaticScrollTop).toBeNull();

    host.scrollTop = 1200;
    host.dispatchEvent(new Event("scroll"));
    expect(terminal._pinnedToBottom).toBe(false);
  } finally {
    vi.unstubAllGlobals();
  }
});

it("keeps following output when the keyboard inset changes without changing the grid", () => {
  const terminal = {
    cols: 80,
    rows: 24,
    autoResize: true,
    _lastContainerRect: { width: 640, height: 390 },
    _measureCharSize: () => ({ charWidth: 8, rowHeight: 16 }),
    _visibleContentHeight: () => 384,
    _setKeyboardInset: vi.fn(),
    _isScrolledToBottom() {
      return this._setKeyboardInset.mock.calls.length === 0;
    },
    _scrollToBottom: vi.fn(),
    onResizeRequest: vi.fn(),
  };

  Reflect.get(WTerm.prototype, "_fitAutoResize").call(terminal);

  expect(terminal._setKeyboardInset).toHaveBeenCalledWith(6);
  expect(terminal.onResizeRequest).toHaveBeenCalledWith(80, 24);
  expect(terminal._scrollToBottom).toHaveBeenCalledOnce();
});
