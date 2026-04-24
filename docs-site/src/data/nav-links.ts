export const navLinks = [
	{ href: '/', label: 'Home' },
	{
		href: '/docs/',
		label: 'Docs',
		activePrefixes: ['/docs/', '/learn/', '/architecture/', '/spec/', '/marketplace/'],
	},
	{ href: '/demo/', label: 'Demo' },
	{ href: '/status/', label: 'Status' },
	{ href: 'https://github.com/armanas/froglet', label: 'GitHub', external: true },
] as const;
