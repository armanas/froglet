// The source editor: a textarea with a gutter of line numbers beside it. It is deliberately small. The rest of the
// playground uses only what Editor lists, so a richer editor could take its place.

export interface Position {
  /** Counted from 1. */
  line: number;
  column: number;
  endLine?: number;
  endColumn?: number;
}

export interface Editor {
  readonly value: string;
  /** Replaces the text. It does not count as the person changing it. */
  set(value: string): void;
  /** Calls back after the person changes the text. */
  onChange(callback: (value: string) => void): void;
  /** Selects a place in the text, scrolls to it, and focuses the editor. */
  reveal(position: Position): void;
  /** Marks lines (from 1) in the gutter, for the ones with a problem. Replaces the earlier marks. */
  mark(lines: number[]): void;
}

export function createEditor({ textarea, gutter }: { textarea: HTMLTextAreaElement; gutter: HTMLElement }): Editor {
  const callbacks: Array<(value: string) => void> = [];
  let marked = new Set<number>();

  const renderGutter = () => {
    const count = textarea.value.split('\n').length;
    while (gutter.childElementCount > count) gutter.lastElementChild!.remove();
    while (gutter.childElementCount < count) {
      const number = gutter.childElementCount + 1;
      const item = document.createElement('span');
      item.textContent = String(number);
      item.classList.toggle('is-marked', marked.has(number));
      gutter.append(item);
    }
  };

  textarea.addEventListener('input', () => {
    renderGutter();
    for (const callback of callbacks) callback(textarea.value);
  });
  // The gutter follows the text as it scrolls.
  textarea.addEventListener('scroll', () => {
    gutter.scrollTop = textarea.scrollTop;
  });
  renderGutter();

  return {
    get value() {
      return textarea.value;
    },
    set(value) {
      textarea.value = value;
      renderGutter();
    },
    onChange: (callback) => void callbacks.push(callback),
    mark(lines) {
      marked = new Set(lines);
      Array.from(gutter.children).forEach((item, index) => item.classList.toggle('is-marked', marked.has(index + 1)));
    },
    reveal({ line, column, endLine, endColumn }) {
      const lines = textarea.value.split('\n');
      const offsetOf = (at: number, inLine: number) => lines.slice(0, at - 1).reduce((offset, text) => offset + text.length + 1, 0) + Math.min(Math.max(inLine - 1, 0), lines[at - 1]?.length ?? 0);
      const start = offsetOf(line, column);
      const end = endLine !== undefined && endColumn !== undefined ? offsetOf(endLine, endColumn) : start;
      textarea.focus();
      textarea.setSelectionRange(start, Math.max(start, end));
      // Show the line a few lines down from the top, so the place has some code around it.
      const lineHeight = parseFloat(getComputedStyle(textarea).lineHeight) || 20;
      textarea.scrollTop = Math.max(0, (line - 3) * lineHeight);
      gutter.scrollTop = textarea.scrollTop;
    },
  };
}
