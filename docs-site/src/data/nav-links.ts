export const navLinks = [
	{ href: '/', label: 'Home' },
	{
		href: '/learn/',
		label: 'Docs',
		activePrefixes: ['/learn/', '/architecture/', '/spec/', '/marketplace/'],
	},
	{ href: 'https://github.com/armanas/froglet', label: 'GitHub', external: true },
] as const;
