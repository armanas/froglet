export interface AgentRouteMetadata {
	route: string;
	page_role: string;
	agent_purpose: string;
	primary_task: string;
	relevant_tasks: string[];
	evidence_boundary: string;
	canonical_sources: string[];
	preferred_next_action: string;
}

export const routeAgentMetadata = {
	home: {
		route: '/',
		page_role: 'entrypoint',
		agent_purpose: 'Choose between hosted proof and local install, then make the agent collect evidence before recommendations.',
		primary_task: 'hosted-proof',
		relevant_tasks: ['hosted-proof', 'hosted-proof-with-witness', 'receipt-feed-check', 'local-install-proposal'],
		evidence_boundary: 'The homepage prompt proves only the public hosted demo until the agent runs the live HTTP flow.',
		canonical_sources: ['/llms.txt', '/agent-tasks.json', '/learn/cloud-trial/'],
		preferred_next_action: 'Open /llms.txt or /agent-tasks.json, run hosted-proof, then report observed statuses and receipt evidence.',
	},
	marketplace: {
		route: '/marketplace/',
		page_role: 'marketplace_snapshot',
		agent_purpose: 'Inspect public marketplace counts and copy an evidence snapshot without overstating hosted paid rails.',
		primary_task: 'marketplace-evidence',
		relevant_tasks: ['marketplace-evidence', 'receipt-feed-check', 'local-install-proposal'],
		evidence_boundary: 'Marketplace counts are observed public index data, not proof that hosted paid rails or mainnet money are live.',
		canonical_sources: ['/marketplace/', '/agent-tasks.json', 'https://marketplace.froglet.dev/v1/providers?limit=12', 'https://marketplace.froglet.dev/v1/offers?limit=24'],
		preferred_next_action: 'Use the copy evidence action, then cite snapshot time, providers, offers, receipts, and source links.',
	},
	demo: {
		route: '/demo/',
		page_role: 'interactive_walkthrough',
		agent_purpose: 'Use the browser walkthrough only as conceptual orientation; use hosted-proof for live evidence.',
		primary_task: 'hosted-proof',
		relevant_tasks: ['hosted-proof', 'local-install-proposal'],
		evidence_boundary: 'The interactive demo is educational and does not replace live hosted or local verification.',
		canonical_sources: ['/demo/', '/llms.txt', '/agent-tasks.json'],
		preferred_next_action: 'After orientation, run hosted-proof instead of summarizing the walkthrough as proof.',
	},
	managed: {
		route: '/managed/',
		page_role: 'coming_soon_boundary',
		agent_purpose: 'Keep managed-product claims separate from the protocol and local/self-hosted paths available today.',
		primary_task: 'local-install-proposal',
		relevant_tasks: ['local-install-proposal', 'hosted-proof'],
		evidence_boundary: 'Managed cloud-hosted nodes are not available today; direct users to hosted proof or local install.',
		canonical_sources: ['/managed/', '/learn/cloud-trial/', '/learn/quickstart/', '/agent-tasks.json'],
		preferred_next_action: 'If the user wants managed features today, report that they are not wired up and offer hosted proof or local install planning.',
	},
	openSource: {
		route: '/open-source/',
		page_role: 'repository_orientation',
		agent_purpose: 'Orient agents toward the public kernel, reference implementation, integrations, and conformance boundaries.',
		primary_task: 'local-install-proposal',
		relevant_tasks: ['local-install-proposal', 'receipt-artifact-verify'],
		evidence_boundary: 'Repository openness does not prove a local install or receipt until the relevant commands or verifiers run.',
		canonical_sources: ['/open-source/', 'https://github.com/armanas/froglet', '/agent-tasks.json'],
		preferred_next_action: 'Use the repo and docs to choose the smallest local verification path for the user context.',
	},
	verifyReceipt: {
		route: '/verify-receipt/',
		page_role: 'receipt_structural_inspector',
		agent_purpose: 'Inspect pasted receipt or feed artifact JSON for expected Froglet receipt fields and explicit verification limits.',
		primary_task: 'receipt-artifact-verify',
		relevant_tasks: ['receipt-artifact-verify', 'receipt-feed-check'],
		evidence_boundary: 'The browser helper checks structure only; it does not verify Schnorr signatures, canonical hashes, provider identity, or settlement.',
		canonical_sources: ['/verify-receipt/', '/agent-tasks.json', '/spec/kernel/'],
		preferred_next_action: 'Paste the artifact JSON, run structural inspection, and report both present fields and cryptographic work not yet performed.',
	},
	privacy: {
		route: '/privacy/',
		page_role: 'privacy_boundary',
		agent_purpose: 'Explain public website, hosted proof, and local/self-hosted privacy boundaries without inferring unobserved behavior.',
		primary_task: 'chat-only-fallback',
		relevant_tasks: ['chat-only-fallback', 'hosted-proof', 'local-install-proposal'],
		evidence_boundary: 'Privacy claims are policy and architecture boundaries unless verified against live routes or local configuration.',
		canonical_sources: ['/privacy/', '/llms.txt', '/agent-tasks.json'],
		preferred_next_action: 'Use this page to bound privacy claims, then run hosted-proof or local checks for runtime evidence.',
	},
	terms: {
		route: '/terms/',
		page_role: 'usage_policy',
		agent_purpose: 'State the terms of service, acceptable use policy, and takedown process for the first-party hosted Froglet services.',
		primary_task: 'chat-only-fallback',
		relevant_tasks: ['chat-only-fallback', 'hosted-proof'],
		evidence_boundary: 'Terms apply to the hosted services only; self-hosted nodes are governed by the Apache-2.0 license, not this page.',
		canonical_sources: ['/terms/', '/privacy/', '/llms.txt'],
		preferred_next_action: 'Use this page to bound usage and takedown claims; file marketplace complaints via the arbiter complaint endpoint.',
	},
} as const satisfies Record<string, AgentRouteMetadata>;
