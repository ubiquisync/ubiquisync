// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	site: 'https://www.ubiquisync.com',
	integrations: [
		starlight({
			title: 'Ubiquisync',
			logo: {
				src: './src/assets/logo.svg',
			},
			customCss: ['./src/styles/theme.css'],
			head: [
				// raster fallbacks for browsers without SVG favicon support
				{ tag: 'link', attrs: { rel: 'icon', href: '/favicon.ico', sizes: '48x48' } },
				{ tag: 'link', attrs: { rel: 'apple-touch-icon', href: '/apple-touch-icon.png' } },
			],
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/ubiquisync/ubiquisync' },
			],
			sidebar: [
				{
					label: 'Guides',
					items: [{ label: 'Getting started', slug: 'guides/getting-started' }],
				},
				{
					label: 'Protocol',
					items: [{ autogenerate: { directory: 'protocol' } }],
				},
			],
		}),
	],
});
