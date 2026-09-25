import { serviceReference } from '../data/shared-service';

const element = (id: string) => document.getElementById(id)!;
const query = new URLSearchParams(location.search);
const provider = query.get('provider') ?? '';
const service = query.get('service') ?? '';
const prompt = element('recipient-prompt') as HTMLTextAreaElement;
let latest: Awaited<ReturnType<typeof import('../data/shared-service').sharedService>> | null = null;
let valid = false;
try {
  serviceReference(provider, service);
  valid = true;
  element('service-title').textContent = service;
  element('service-provider').textContent = provider;
  const siteOrigin = location.origin || 'https://froglet.dev';
  const link = new URL('/service/', siteOrigin);
  link.searchParams.set('provider', provider); link.searchParams.set('service', service);
  prompt.value = `Use this Froglet service: ${link}. Read ${new URL('/learn/share-services/', siteOrigin)} first. If Froglet is missing, prepare its native installation and ask me to approve it. Verify the provider identity and signed offer; make one free call using the current example input and report the result, receipt verification, and anything not verified. Do not accept a paid offer.`;
} catch (error) {
  element('availability').textContent = String(error);
  (element('copy-service-prompt') as HTMLButtonElement).disabled = true;
  (element('refresh-service') as HTMLButtonElement).disabled = true;
}
function render() {
  if (!latest) return;
  const expired = latest.leaseExpiresAt !== null && latest.leaseExpiresAt * 1000 <= Date.now();
  element('service-summary').textContent = latest.summary;
  element('service-revision').textContent = latest.revision ?? 'No immutable revision reported';
  element('service-marketplace').textContent = latest.marketplace === 'active' && !expired ? 'Active at the last check' : 'Pending, offline, or not verified';
  element('service-example').textContent = JSON.stringify(latest.exampleInput, null, 2);
  element('checked-at').textContent = `Last successful check: ${new Date(latest.checkedAt).toLocaleString()}`;
}
async function refresh() {
  if (!valid) return;
  const button = element('refresh-service') as HTMLButtonElement;
  if (button.disabled) return;
  button.disabled = true;
  element('availability').textContent = 'Checking current availability…';
  try {
    const response = await fetch(`/api/shared-service?${new URLSearchParams({ provider, service })}`, { cache: 'no-store', signal: AbortSignal.timeout(15000) });
    if (!response.ok) throw new Error('Service unavailable');
    latest = await response.json();
    render();
    element('availability').textContent = latest?.free ? 'Provider reachable. Your agent still needs to make its own verified call.' : 'Provider reachable, but free pricing is not confirmed. Do not run a paid call through this journey.';
  } catch {
    render();
    element('availability').textContent = latest ? 'STALE — the service could not be checked. The details below are from the last successful check.' : 'Availability not verified. The publisher may be asleep, offline, or no longer publishing. Ask them to check Froglet status.';
    element('service-marketplace').textContent = 'Current activation not verified';
  } finally { button.disabled = false; }
}
element('refresh-service').addEventListener('click', refresh);
element('copy-service-prompt').addEventListener('click', async () => {
  try { await navigator.clipboard.writeText(prompt.value); element('copy-status').textContent = 'Prompt copied.'; }
  catch { prompt.focus(); prompt.select(); element('copy-status').textContent = 'Select and copy the prompt above using your device’s copy command.'; }
});
void refresh();
setInterval(() => { if (!document.hidden) void refresh(); }, 30000);
