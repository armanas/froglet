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
  'lightning-mock': '# After MCP status passes, ask froglet-mcp for the Lightning mock payment setup.',
  'lightning-lnd-rest': '# After MCP status passes, configure LND REST with your LND URL, macaroon, and TLS cert.',
  'stripe-test': '# After MCP status passes, configure Stripe test mode with sk_test_... and webhook proof.',
  'stripe-live': '# After MCP status passes, configure Stripe live mode only after a fresh live-payment approval.',
  x402: '# After MCP status passes, configure x402 with your Base wallet address and facilitator.',
};

export function buildSelfHostScript(config: SelfHostConfig = DEFAULT_SELF_HOST_CONFIG): string {
  const lines: string[] = [];
  const env: string[] = [];
  if (config.agent !== 'claude-code') env.push(`FROGLET_AGENT_TARGET=${config.agent}`);
  lines.push(`${env.length > 0 ? `${env.join(' ')} ` : ''}curl -fsSL https://froglet.dev/agent | bash`);
  if (PAYMENT_NOTES[config.payment]) lines.push(PAYMENT_NOTES[config.payment]);

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

  if (!card || !output) return;

  const state: SelfHostConfig = { ...DEFAULT_SELF_HOST_CONFIG };

  function render(): void {
    const code = output?.querySelector('code');
    if (code) code.textContent = buildSelfHostScript(state);
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
        render();
      });
    });
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
