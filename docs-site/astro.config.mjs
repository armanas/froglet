// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import remarkMath from 'remark-math';
import rehypeMathjax from 'rehype-mathjax';

const site = process.env.SITE_URL ?? 'https://froglet.dev';

export default defineConfig({
	site,
	markdown: {
		remarkPlugins: [remarkMath],
		rehypePlugins: [rehypeMathjax],
	},
	integrations: [
			starlight({
				title: '\uD83D\uDC38 Froglet',
				description: 'Identity. Execution. Settlement.',
				disable404Route: true,
				social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/armanas/froglet' }],
			head: [
				{ tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.googleapis.com' } },
				{ tag: 'link', attrs: { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: true } },
				{ tag: 'link', attrs: { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Source+Sans+3:wght@400;500;600;700&family=Source+Serif+4:wght@400;500;600;700&family=JetBrains+Mono:wght@400;600&display=swap' } },
			],
			customCss: ['./src/styles/custom.css'],
			components: {
				ThemeProvider: './src/components/ThemeProvider.astro',
				Header: './src/components/StarlightHeader.astro',
				Footer: './src/components/StarlightFooter.astro',
				Sidebar: './src/components/StarlightSidebar.astro',
			},
				sidebar: [
					{
						label: '📖 Learn',
						items: [
							{ label: '🗺️ Overview', slug: 'learn' },
							{ label: '⚡ Quickstart', slug: 'learn/quickstart' },
							{ label: '☁️ Try In Cloud', slug: 'learn/cloud-trial' },
							{ label: '🤖 Connect Agents', slug: 'learn/agents' },
								{ label: '🦾 LLM Self-Install', slug: 'learn/llm-self-install' },
							{ label: '💳 Payment Rails', slug: 'learn/payment-rails' },
							{ label: '⚡ Lightning', slug: 'learn/payment-lightning' },
							{ label: '💳 Stripe', slug: 'learn/payment-stripe' },
							{ label: '🔗 x402', slug: 'learn/payment-x402' },
							{ label: '❓ What is Froglet?', slug: 'learn/introduction' },
							{ label: '🔑 Cryptographic Identity', slug: 'learn/identity' },
							{ label: '🔄 The Deal Flow', slug: 'learn/deal-flow' },
							{ label: '🤝 Settlement', slug: 'learn/settlement' },
							{ label: '🎯 Trust & Economics', slug: 'learn/economics' },
						],
					},
					{
						label: '🏪 Marketplace',
						items: [
							{ label: '🏪 Marketplace', slug: 'marketplace/overview' },
						],
					},
					{
						label: '📐 Specification',
						items: [
							{ label: '⚙️ Kernel', slug: 'spec/kernel' },
							{ label: '🔌 Service Binding', slug: 'spec/service-binding' },
						],
					},
					{
						label: '🏗️ Architecture',
						items: [
							{ label: '🏗️ Overview', slug: 'architecture/overview' },
							{ label: '📦 Crate Structure', slug: 'architecture/crates' },
							{ label: '📁 File Reference', slug: 'architecture/files' },
						],
					},
			],
		}),
	],
});
