// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import remarkMath from 'remark-math';
import rehypeMathjax from 'rehype-mathjax';

const site = process.env.SITE_URL ?? 'https://armanas.dev';

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
				{ tag: 'link', attrs: { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;600&display=swap' } },
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
						label: 'Learn',
						items: [
							{ label: 'Overview', slug: 'learn' },
							{ label: 'Quickstart', slug: 'learn/quickstart' },
							{ label: 'What is Froglet?', slug: 'learn/introduction' },
							{ label: 'Cryptographic Identity', slug: 'learn/identity' },
							{ label: 'The Deal Flow', slug: 'learn/deal-flow' },
							{ label: 'Settlement', slug: 'learn/settlement' },
							{ label: 'Trust & Economics', slug: 'learn/economics' },
						],
					},
					{
						label: 'Marketplace',
						items: [
							{ label: 'Marketplace', slug: 'marketplace/overview' },
						],
					},
					{
						label: 'Specification',
						items: [
							{ label: 'Kernel', slug: 'spec/kernel' },
							{ label: 'Service Binding', slug: 'spec/service-binding' },
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
			],
		}),
	],
});
