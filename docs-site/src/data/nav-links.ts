export const navLinks = [
	{ href: '/', label: 'Home', hideOnMobile: true },
	{
		href: '/docs/',
		label: 'Docs',
		activePrefixes: ['/docs/', '/learn/', '/architecture/', '/spec/', '/marketplace/overview/'],
	},
	{ href: '/marketplace/', label: 'Marketplace' },
	{ href: '/managed/', label: 'Managed' },
	{ href: '/open-source/', label: 'Open source' },
] as const;
