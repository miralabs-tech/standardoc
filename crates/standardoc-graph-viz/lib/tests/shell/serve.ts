// Minimal bun dev server for the Playwright shell harness. Serves
// `harness.html` at `/` and bundles `harness.ts` (→ `/harness.js`) on
// demand with the same SCSS plugin the playground uses, so the lib's
// `.scss` + `.module.scss` imports resolve in the browser bundle.
//
// Launched by `playwright.config.ts` as the test webServer.

import * as crypto from 'node:crypto';
import * as path from 'node:path';

import type { BunPlugin } from 'bun';

const PORT = Number(process.env.HARNESS_PORT ?? 4321);
const DIR = path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1'));

let sassMod: Promise<typeof import('sass')> | null = null;
function loadSass(): Promise<typeof import('sass')> {
  if (sassMod === null) sassMod = import('sass');
  return sassMod;
}

const scssClassRegex = /\.([A-Za-z_][\w-]*)/g;
function shortHash(input: string): string {
  return crypto.createHash('sha1').update(input).digest('hex').slice(0, 8);
}
function styleInjector(css: string, abs: string, exportObject: string): string {
  const id = `sd-scss-${shortHash(abs)}`;
  return [
    `const __css = ${JSON.stringify(css)};`,
    `if (typeof document !== "undefined" && !document.getElementById(${JSON.stringify(id)})) {`,
    `  const el = document.createElement("style");`,
    `  el.id = ${JSON.stringify(id)};`,
    `  el.textContent = __css;`,
    `  document.head.appendChild(el);`,
    `}`,
    `export default ${exportObject};`,
  ].join('\n');
}

const scssModulesPlugin: BunPlugin = {
  name: 'scss-modules',
  setup(build) {
    build.onLoad({ filter: /\.scss$/ }, async args => {
      const abs = path.resolve(args.path);
      const isModule = abs.endsWith('.module.scss');
      const sass = await loadSass();
      const result = sass.compile(abs, { style: 'expanded', loadPaths: [path.dirname(abs)] });
      if (isModule) {
        const classes: Record<string, string> = {};
        const hash = shortHash(abs);
        const css = result.css.replace(scssClassRegex, (_m, name: string) => {
          const hashed = `${name}_${hash}`;
          classes[name] = hashed;
          return `.${hashed}`;
        });
        return { contents: styleInjector(css, abs, JSON.stringify(classes)), loader: 'js' };
      }
      return { contents: styleInjector(result.css, abs, JSON.stringify('')), loader: 'js' };
    });
  },
};

async function bundleHarness(): Promise<Response> {
  const built = await Bun.build({
    entrypoints: [path.join(DIR, 'harness.ts')],
    target: 'browser',
    format: 'esm',
    sourcemap: 'inline',
    plugins: [scssModulesPlugin],
  });
  if (!built.success) {
    const log = built.logs.map(l => l.message).join('\n');
    return new Response(`/* build failed:\n${log}\n*/`, {
      status: 500,
      headers: { 'Content-Type': 'application/javascript; charset=utf-8' },
    });
  }
  const out = built.outputs[0];
  const body = out === undefined ? '/* no output */' : await out.text();
  return new Response(body, {
    headers: { 'Content-Type': 'application/javascript; charset=utf-8', 'Cache-Control': 'no-store' },
  });
}

const server = Bun.serve({
  port: PORT,
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === '/harness.js') return bundleHarness();
    if (url.pathname === '/' || url.pathname === '/harness.html') {
      return new Response(Bun.file(path.join(DIR, 'harness.html')), {
        headers: { 'Content-Type': 'text/html; charset=utf-8' },
      });
    }
    return new Response('Not found', { status: 404 });
  },
});

console.log(`[harness] http://${server.hostname}:${server.port}`);
