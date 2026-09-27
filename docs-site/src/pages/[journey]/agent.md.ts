import type { APIRoute } from 'astro';
import publishGuide from '../../content/docs/learn/share-services.mdx?raw';
import installGuide from '../../content/docs/learn/quickstart.mdx?raw';

// Generate machine-readable views from the human guides, so commands and
// release constraints cannot diverge between the two entry paths.
export function getStaticPaths() {
  return [
    { params: { journey: 'publish' }, props: { guide: publishGuide } },
    { params: { journey: 'install' }, props: { guide: installGuide } },
  ];
}

export const GET: APIRoute = ({ props }) => {
  const origin = import.meta.env.FROGLET_SITE_ORIGIN || 'https://froglet.dev';
  const guide = props.guide as string;
  const title = /^title: "(.+)"$/m.exec(guide)?.[1] ?? 'Froglet agent guide';
  const body = guide.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n/, '')
    .replace(/\]\(\/(?!\/)([^)]*)\)/g, (_match, path: string) => `](${origin}/${path})`);
  return new Response(`# ${title}\n${body}`, {
    headers: { 'content-type': 'text/markdown; charset=utf-8' },
  });
};
