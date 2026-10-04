// A standalone deployment keeps the full site and its assets ready for restoration.
const favicon = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><text x="32" y="40" text-anchor="middle" font-family="monospace" font-weight="700" font-size="22" fill="#397340" letter-spacing="-0.5">_o..o_</text></svg>`;

const page = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Coming back soon — Froglet</title>
  <meta name="description" content="Sorry for any inconvenience. Thanks for stopping by.">
  <meta name="theme-color" content="#f7f8f2">
  <meta property="og:title" content="Froglet — Coming back soon">
  <meta property="og:description" content="Sorry for any inconvenience. Thanks for stopping by.">
  <meta property="og:type" content="website">
  <meta name="twitter:card" content="summary">
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <style>
    :root { color-scheme: light; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; color: #203628; background: #f7f8f2; }
    * { box-sizing: border-box; }
    body { margin: 0; min-height: 100vh; min-height: 100svh; display: grid; grid-template-rows: auto 1fr auto; }
    header, footer { padding: 32px clamp(24px, 6vw, 88px); }
    header { font-size: 21px; font-weight: 650; letter-spacing: -.6px; }
    main { padding: 56px 24px 80px; text-align: center; align-self: center; }
    .mark { display: inline-block; margin-bottom: 28px; color: #397340; font: 700 clamp(48px, 9vw, 76px)/1.2 ui-monospace, "SF Mono", Menlo, Consolas, monospace; letter-spacing: -4px; }
    h1 { margin: 0; font-size: clamp(38px, 7vw, 76px); font-weight: 550; line-height: 1.12; letter-spacing: -.05em; }
    p { max-width: 360px; margin: 24px auto 0; color: #56645b; font-size: 18px; line-height: 1.65; }
    footer { color: #637168; font-size: 13px; text-align: center; }
    @media (max-width: 480px) { header, footer { padding-block: 24px; } main { padding-block: 40px 64px; } p { font-size: 16px; max-width: 290px; } }
  </style>
</head>
<body>
  <header>froglet</header>
  <main>
    <div class="mark" aria-hidden="true">_o..o_</div>
    <h1>Coming back soon.</h1>
    <p>Sorry for any inconvenience. Thanks for stopping by.</p>
  </main>
  <footer>froglet.dev</footer>
</body>
</html>`;

export default {
  fetch(request: Request): Response {
    const isIcon = new URL(request.url).pathname === '/favicon.svg'
      && (request.method === 'GET' || request.method === 'HEAD');
    return new Response(request.method === 'HEAD' ? null : isIcon ? favicon : page, {
      status: isIcon ? 200 : 503,
      headers: {
        'content-type': isIcon ? 'image/svg+xml; charset=utf-8' : 'text/html; charset=utf-8',
        'cache-control': 'no-store',
        ...(isIcon ? {} : { 'retry-after': '3600' }),
        'content-security-policy': "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        'x-content-type-options': 'nosniff',
        'referrer-policy': 'no-referrer',
      },
    });
  },
};
