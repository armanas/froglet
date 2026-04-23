export type InstallTarget = 'linux' | 'linux-arm' | 'macos' | 'docker';
export type AgentTarget = 'claude-code' | 'codex' | 'openclaw';
export type PaymentRail = 'lightning' | 'stripe' | 'x402';

export interface SelfHostConfig {
  install: InstallTarget;
  agent: AgentTarget;
  payment: PaymentRail;
}

export const DEFAULT_SELF_HOST_CONFIG: SelfHostConfig = {
  install: 'linux',
  agent: 'claude-code',
  payment: 'lightning',
};

const INSTALL_COMMANDS: Record<Exclude<InstallTarget, 'docker'>, string> = {
  linux: 'curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh',
  'linux-arm':
    'curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | ARCH=arm64 sh',
  macos: 'curl -fsSL https://raw.githubusercontent.com/armanas/froglet/main/scripts/install.sh | sh',
};

const PAYMENT_COMMANDS: Record<PaymentRail, string> = {
  lightning: './scripts/setup-payment.sh lightning',
  stripe: 'FROGLET_STRIPE_SECRET_KEY=<stripe-test-secret-key> ./scripts/setup-payment.sh stripe',
  x402: 'FROGLET_X402_WALLET_ADDRESS=<base-wallet-address> ./scripts/setup-payment.sh x402',
};

export function buildSelfHostScript(config: SelfHostConfig = DEFAULT_SELF_HOST_CONFIG): string {
  const lines: string[] = [];

  if (config.install !== 'docker') {
    lines.push(INSTALL_COMMANDS[config.install]);
  }

  lines.push(
    'git clone https://github.com/armanas/froglet.git',
    'cd froglet',
    `./scripts/setup-agent.sh --target ${config.agent}`,
    PAYMENT_COMMANDS[config.payment],
    `set -a && . ./.froglet/payment/${config.payment}.env && export FROGLET_HOST_READABLE_CONTROL_TOKEN=true && set +a`,
    'docker compose up --build -d',
  );

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
