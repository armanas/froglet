import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { buildSelfHostScript, initSelfHostConfigurator } from '../self-host-configurator';

describe('buildSelfHostScript', () => {
  it('builds a pasteable default script without nested cd commands', () => {
    const script = buildSelfHostScript();

    expect(script).toContain('git clone https://github.com/armanas/froglet.git\ncd froglet\n');
    expect(script.match(/^cd froglet$/gm)).toHaveLength(1);
    expect(script).not.toContain('cd froglet &&');
    expect(script).toContain('./scripts/setup-agent.sh --target claude-code');
    expect(script).toContain('./scripts/setup-payment.sh lightning');
    expect(script).toContain('docker compose up --build -d');
  });

  it('adds the Linux arm64 installer environment', () => {
    const script = buildSelfHostScript({
      install: 'linux-arm',
      agent: 'codex',
      payment: 'x402',
    });

    expect(script).toContain('| ARCH=arm64 sh');
    expect(script).toContain('./scripts/setup-agent.sh --target codex');
    expect(script).toContain('FROGLET_X402_WALLET_ADDRESS=<base-wallet-address>');
  });

  it('omits the install curl command for Docker-only setup', () => {
    const script = buildSelfHostScript({
      install: 'docker',
      agent: 'openclaw',
      payment: 'stripe',
    });

    expect(script).not.toContain('install.sh');
    expect(script).toContain('./scripts/setup-agent.sh --target openclaw');
    expect(script).toContain('FROGLET_STRIPE_SECRET_KEY=<stripe-test-secret-key>');
  });
});

describe('initSelfHostConfigurator', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    document.body.innerHTML = '';
  });

  it('renders and updates the selected config with aria state', () => {
    document.body.innerHTML = `
      <div id="self-host-card">
        <div class="config-options" data-group="install">
          <button class="config-btn is-active" data-value="linux" aria-pressed="true">Linux</button>
          <button class="config-btn" data-value="docker" aria-pressed="false">Docker</button>
        </div>
        <div class="config-options" data-group="agent">
          <button class="config-btn is-active" data-value="claude-code" aria-pressed="true">Claude Code</button>
          <button class="config-btn" data-value="codex" aria-pressed="false">Codex</button>
        </div>
        <div class="config-options" data-group="payment">
          <button class="config-btn is-active" data-value="lightning" aria-pressed="true">Lightning</button>
          <button class="config-btn" data-value="stripe" aria-pressed="false">Stripe</button>
        </div>
      </div>
      <pre id="config-output"><code></code></pre>
      <button id="config-copy-btn">Copy</button>
    `;

    initSelfHostConfigurator();

    const dockerButton = document.querySelector<HTMLButtonElement>('[data-value="docker"]');
    const linuxButton = document.querySelector<HTMLButtonElement>('[data-value="linux"]');
    dockerButton?.click();

    expect(dockerButton?.getAttribute('aria-pressed')).toBe('true');
    expect(linuxButton?.getAttribute('aria-pressed')).toBe('false');
    expect(document.querySelector('#config-output code')?.textContent).not.toContain('install.sh');
  });

  it('copies the rendered script', async () => {
    document.body.innerHTML = `
      <div id="self-host-card"></div>
      <pre id="config-output"><code>hello</code></pre>
      <button id="config-copy-btn">Copy</button>
    `;

    initSelfHostConfigurator();
    document.querySelector<HTMLButtonElement>('#config-copy-btn')?.click();
    await vi.advanceTimersByTimeAsync(0);

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(buildSelfHostScript());
    expect(document.querySelector('#config-copy-btn')?.textContent).toBe('Copied');
  });
});
