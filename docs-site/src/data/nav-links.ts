export const navLinks = [
	{ href: '/publish/', label: 'Start sharing' },
	{ href: '/marketplace/', label: 'Explore services' },
	{ href: '/managed/', label: 'For organizations' },
	{ href: '/open-source/', label: 'Developers' },
	{
		href: '/docs/',
		label: 'Docs',
		activePrefixes: ['/docs/', '/learn/', '/architecture/', '/spec/', '/marketplace/overview/'],
	},
] as const;
