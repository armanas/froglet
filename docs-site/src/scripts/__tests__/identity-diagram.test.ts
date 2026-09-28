import { describe, expect, it } from 'vitest';
import { ELEMENTS, initIdentityDiagram } from '../identity-diagram';

describe('identity explanation controls', () => {
  it('updates the explanation and exposes exactly one selected button', () => {
    document.body.innerHTML = '<div id="diagram"><button data-identity-index="0" aria-pressed="true">Private Key</button><button data-identity-index="1" aria-pressed="false"><strong>Public Key</strong></button><button data-identity-index="2" aria-pressed="false">Node ID</button></div><div id="detail" aria-live="polite"></div>';
    const diagram = document.getElementById('diagram')!;
    const detail = document.getElementById('detail')!;
    initIdentityDiagram(diagram, detail);
    const buttons = Array.from(diagram.querySelectorAll('button'));
    buttons[1].querySelector('strong')!.click();
    expect(detail.textContent).toContain(ELEMENTS[1].detail);
    expect(buttons.map(button => button.getAttribute('aria-pressed'))).toEqual(['false', 'true', 'false']);
    buttons[2].click();
    expect(detail.textContent).toContain('not a verified person or organization');
    expect(buttons.map(button => button.getAttribute('aria-pressed'))).toEqual(['false', 'false', 'true']);
  });
});
