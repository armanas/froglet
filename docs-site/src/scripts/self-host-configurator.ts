export type InstallTarget = 'linux' | 'linux-arm' | 'macos' | 'docker';
export type AgentTarget = 'claude-code' | 'codex' | 'manual';
export type PaymentRail = 'none' | 'lightning-mock' | 'lightning-lnd-rest' | 'stripe-test' | 'stripe-live' | 'x402';

export interface SelfHostConfig {
  install: InstallTarget;
  agent: AgentTarget;
  payment: PaymentRail;
}

export const DEFAULT_SELF_HOST_CONFIG: SelfHostConfig = {
  install: 'linux',
  agent: 'claude-code',
  payment: 'none',
};

const PAYMENT_NOTES: Record<PaymentRail, string> = {
  none: '',
  'lightning-mock': '# configure Lightning mock payment setup locally\nexport FROGLET_PAYMENT_BACKEND=lightning\nexport FROGLET_LIGHTNING_MODE=mock',
  'lightning-lnd-rest': '# After MCP status passes, configure LND REST with your LND URL, macaroon, and TLS cert.',
  'stripe-test': '# configure Stripe test mode with sk_test_... and webhook proof.\nexport FROGLET_PAYMENT_BACKEND=stripe\nexport FROGLET_STRIPE_SECRET_KEY=sk_test_...',
  'stripe-live': '# After MCP status passes, configure Stripe live mode only after a fresh live-payment approval.',
  x402: '# configure x402 with your Base wallet address and facilitator.\nexport FROGLET_PAYMENT_BACKEND=x402\nexport FROGLET_X402_WALLET_ADDRESS=0x...',
};

function getPaymentNote(payment: PaymentRail, cred?: string): string {
  if (payment === 'stripe-test' && cred) {
    return `# configure Stripe test mode with sk_test_... and webhook proof.\nexport FROGLET_PAYMENT_BACKEND=stripe\nexport FROGLET_STRIPE_SECRET_KEY=${cred}`;
  }
  if (payment === 'x402' && cred) {
    return `# configure x402 with your Base wallet address and facilitator.\nexport FROGLET_PAYMENT_BACKEND=x402\nexport FROGLET_X402_WALLET_ADDRESS=${cred}`;
  }
  return PAYMENT_NOTES[payment] || '';
}

export function buildSelfHostScript(config: SelfHostConfig = DEFAULT_SELF_HOST_CONFIG, credential?: string): string {
  const lines: string[] = [];
  const env: string[] = [];

  const note = getPaymentNote(config.payment, credential);
  if (note) {
    lines.push(note);
  }

  if (config.agent !== 'claude-code') {
    env.push(`FROGLET_AGENT_TARGET=${config.agent}`);
  }

  lines.push(`${env.length > 0 ? `${env.join(' ')} ` : ''}curl -fsSL https://froglet.dev/agent | bash`);

  return lines.join('\n');
}

function setGroupValue(group: HTMLElement, value: string): void {
  group.querySelectorAll<HTMLButtonElement>('.config-btn').forEach((button) => {
    const isActive = button.dataset.value === value;
    button.classList.toggle('is-active', isActive);
    button.setAttribute('aria-pressed', String(isActive));
  });
}

export function initSelfHostConfigurator(root: Document | HTMLElement = document): void {
  const card = root.querySelector<HTMLElement>('#self-host-card');
  const output = root.querySelector<HTMLElement>('#config-output');
  const copyButton = root.querySelector<HTMLButtonElement>('#config-copy-btn');
  const inputContainer = root.querySelector<HTMLElement>('#config-payment-input-container');
  const inputLabel = root.querySelector<HTMLElement>('#config-payment-input-label');
  const inputField = root.querySelector<HTMLInputElement>('#config-payment-input');

  if (!card || !output) return;

  const state: SelfHostConfig = { ...DEFAULT_SELF_HOST_CONFIG };

  function updateInputVisibility(): void {
    if (!inputContainer || !inputLabel || !inputField) return;

    if (state.payment === 'stripe-test') {
      inputContainer.style.display = 'block';
      inputLabel.textContent = 'Stripe Secret Key';
      inputField.placeholder = 'sk_test_...';
    } else if (state.payment === 'x402') {
      inputContainer.style.display = 'block';
      inputLabel.textContent = 'Base Wallet Address';
      inputField.placeholder = '0x...';
    } else {
      inputContainer.style.display = 'none';
      inputField.value = '';
    }
  }

  function render(): void {
    const code = output?.querySelector('code');
    if (code) {
      const cred = inputField?.value?.trim() || '';
      code.textContent = buildSelfHostScript(state, cred);
    }
  }

  card.querySelectorAll<HTMLElement>('.config-options').forEach((group) => {
    const groupName = group.dataset.group as keyof SelfHostConfig | undefined;
    if (!groupName) return;

    setGroupValue(group, state[groupName]);

    group.querySelectorAll<HTMLButtonElement>('.config-btn').forEach((button) => {
      button.addEventListener('click', (event) => {
        const nextValue = button.dataset.value;
        if (!nextValue) return;

        event.preventDefault();
        button.blur();
        state[groupName] = nextValue as SelfHostConfig[typeof groupName];
        setGroupValue(group, nextValue);
        updateInputVisibility();
        render();
      });
    });
  });

  inputField?.addEventListener('input', () => {
    render();
  });

  copyButton?.addEventListener('click', async () => {
    const code = output.querySelector('code');
    const originalLabel = copyButton.textContent || 'Copy';

    try {
      await navigator.clipboard.writeText(code?.textContent || '');
      copyButton.textContent = 'Copied';
    } catch {
      copyButton.textContent = 'Failed';
    }

    setTimeout(() => {
      copyButton.textContent = originalLabel;
    }, 1500);
  });

  render();
}
