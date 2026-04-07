// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import remarkMath from 'remark-math';
import rehypeMathjax from 'rehype-mathjax';

export default defineConfig({
	markdown: {
		remarkPlugins: [remarkMath],
		rehypePlugins: [rehypeMathjax],
	},
	integrations: [
		starlight({
			title: '\uD83D\uDC38 Froglet',
			description: 'Identity. Execution. Settlement.',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/armanas/froglet' }],
			customCss: ['./src/styles/custom.css'],
			sidebar: [
				{
					label: 'Learn',
					items: [
						{ label: 'What is Froglet?', slug: 'learn/introduction' },
						{ label: 'Foundations', slug: 'learn/foundations' },
						{ label: 'Cryptographic Identity', slug: 'learn/identity' },
						{ label: 'Canonical Serialization', slug: 'learn/canonical' },
						{ label: 'Signed Artifacts', slug: 'learn/artifacts' },
						{ label: 'The Deal Flow', slug: 'learn/deal-flow' },
						{ label: 'Settlement (Lightning)', slug: 'learn/settlement' },
						{ label: 'Execution', slug: 'learn/execution' },
						{ label: 'The Network', slug: 'learn/network' },
						{ label: 'Storage & Databases', slug: 'learn/storage' },
						{ label: 'Trust & Verification', slug: 'learn/trust' },
						{ label: 'Economic Model', slug: 'learn/economics' },
					],
				},
				{
					label: 'Architecture',
					items: [
						{ label: 'Overview', slug: 'architecture/overview' },
						{ label: 'Crate Structure', slug: 'architecture/crates' },
						{ label: 'File Reference', slug: 'architecture/files' },
					],
				},
				{
					label: 'Marketplace',
					items: [
						{ label: 'How It Works', slug: 'marketplace/overview' },
						{ label: 'Handlers', slug: 'marketplace/handlers' },
						{ label: 'Indexer', slug: 'marketplace/indexer' },
					],
				},
				{
					label: 'Specification',
					items: [
						{ label: 'Kernel', slug: 'spec/kernel' },
						{ label: 'Service Binding', slug: 'spec/service-binding' },
					],
				},
			],
		}),
	],
});
