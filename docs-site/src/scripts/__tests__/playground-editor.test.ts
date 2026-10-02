import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createEditor, type Editor } from '../playground/editor';

let textarea: HTMLTextAreaElement;
let gutter: HTMLElement;
let editor: Editor;
const numbers = () => Array.from(gutter.children).map((item) => item.textContent);
const marked = () => Array.from(gutter.children).flatMap((item, index) => (item.classList.contains('is-marked') ? [index + 1] : []));
const type = (value: string) => {
  textarea.value = value;
  textarea.dispatchEvent(new Event('input', { bubbles: true }));
};

beforeEach(() => {
  document.body.innerHTML = '<div id="gutter"></div><textarea id="source"></textarea>';
  textarea = document.querySelector('#source')!;
  gutter = document.querySelector('#gutter')!;
  editor = createEditor({ textarea, gutter });
});

describe('the source editor', () => {
  it('numbers the lines of what is in it, and follows as lines are added and taken away', () => {
    expect(numbers()).toEqual(['1']);
    editor.set('a\nb\nc');
    expect(numbers()).toEqual(['1', '2', '3']);
    type('a\nb\nc\nd\n');
    expect(numbers()).toEqual(['1', '2', '3', '4', '5']);
    type('only');
    expect(numbers()).toEqual(['1']);
    expect(editor.value).toBe('only');
  });

  it('tells its listeners when the person changes the text, and not when the page does', () => {
    const changed = vi.fn();
    editor.onChange(changed);
    editor.set('from the page');
    expect(changed).not.toHaveBeenCalled();
    type('from a person');
    expect(changed).toHaveBeenCalledExactlyOnceWith('from a person');
  });

  it('marks the lines it is told about, and keeps the marks on lines that are added later', () => {
    editor.set('a\nb\nc');
    editor.mark([2, 3]);
    expect(marked()).toEqual([2, 3]);
    editor.mark([1]);
    expect(marked()).toEqual([1]);
    type('a\nb\nc\nd');
    expect(marked()).toEqual([1]);
    editor.mark([4]);
    expect(marked()).toEqual([4]);
    editor.mark([]);
    expect(marked()).toEqual([]);
  });

  it('keeps the gutter beside the text as it scrolls', () => {
    editor.set(Array.from({ length: 50 }, (_, index) => `line ${index}`).join('\n'));
    textarea.scrollTop = 120;
    textarea.dispatchEvent(new Event('scroll'));
    expect(gutter.scrollTop).toBe(textarea.scrollTop);
  });

  it('selects a place from a line and column, and puts the cursor in the editor', () => {
    editor.set('first line\nsecond line\nthird line');
    editor.reveal({ line: 2, column: 8, endLine: 2, endColumn: 12 });
    expect(document.activeElement).toBe(textarea);
    expect(textarea.value.slice(textarea.selectionStart, textarea.selectionEnd)).toBe('line');
    editor.reveal({ line: 3, column: 1 });
    expect(textarea.selectionStart).toBe(textarea.selectionEnd);
    expect(textarea.selectionStart).toBe('first line\nsecond line\n'.length);
  });

  it('does not select past the end of a line or of the text', () => {
    editor.set('ab\ncd');
    editor.reveal({ line: 1, column: 99, endLine: 9, endColumn: 9 });
    expect(textarea.selectionStart).toBe(2);
    expect(textarea.selectionEnd).toBeLessThanOrEqual(textarea.value.length);
    editor.reveal({ line: 9, column: 1 });
    expect(textarea.selectionStart).toBeLessThanOrEqual(textarea.value.length);
  });

  it('scrolls to show the place, a few lines down from the top', () => {
    Object.defineProperty(window, 'getComputedStyle', { configurable: true, value: () => ({ lineHeight: '20px' }) });
    editor.set(Array.from({ length: 100 }, (_, index) => `line ${index}`).join('\n'));
    editor.reveal({ line: 40, column: 1 });
    expect(textarea.scrollTop).toBe(37 * 20);
    expect(gutter.scrollTop).toBe(textarea.scrollTop);
    editor.reveal({ line: 1, column: 1 });
    expect(textarea.scrollTop).toBe(0);
  });
});
