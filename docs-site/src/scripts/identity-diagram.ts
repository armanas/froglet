/** Content and accessible interaction for the identity diagram. */

export interface IdentityElement {
  label: string;
  sub: string;
  color: string;
  detail: string;
}

export const ELEMENTS: IdentityElement[] = [
  {
    label: 'Private Key',
    sub: '256-bit secret',
    color: '#e54848',
    detail: 'A securely generated secret in the valid secp256k1 key range, encoded in 32 bytes. Used to sign artifacts. Keep it private.',
  },
  {
    label: 'Public Key',
    sub: 'curve point',
    color: '#4ea3ff',
    detail: 'Computed as P = k · G on secp256k1. Recovering the secret is considered computationally infeasible. Used to verify signatures, not decrypt messages.',
  },
  {
    label: 'Node ID',
    sub: '64-char hex',
    color: '#52c72a',
    detail: 'The 32-byte x-only public key encoded as 64 lowercase hex characters. Identifies a signing key, not a verified person or organization.',
  },
];

export function initIdentityDiagram(container: HTMLElement, detailEl: HTMLElement): void {
  const buttons = Array.from(container.querySelectorAll<HTMLButtonElement>('button[data-identity-index]'));
  for (const button of buttons) {
    button.addEventListener('click', () => {
      const selected = Number(button.dataset.identityIndex);
      const element = ELEMENTS[selected];
      if (!element) return;
      for (const candidate of buttons) {
        candidate.setAttribute('aria-pressed', String(candidate === button));
      }
      const title = document.createElement('strong');
      title.textContent = element.label;
      const explanation = document.createElement('p');
      explanation.textContent = element.detail;
      detailEl.replaceChildren(title, explanation);
    });
  }
}
