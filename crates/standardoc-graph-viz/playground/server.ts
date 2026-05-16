/**
 * Standalone playground dev server for the standardoc-graph-viz WASM
 * engine. Two responsibilities:
 *
 *   1. Serve the static playground files + the `pkg/` wasm artefacts.
 *   2. Proxy `/mcp` requests to the actual standardoc daemon, whose
 *      URL is discovered by reading `<workspace>/.standardoc/mcp.endpoint`
 *      at proxy time (so a daemon restart on a new ephemeral port is
 *      picked up without restarting the playground).
 *
 * The workspace defaults to the repository root (../../..), so if you
 * have VSCode open on this repo and the extension's daemon is running,
 * `bun run dev` "just works". Override via `STANDARDOC_WORKSPACE=/abs/path`.
 *
 * The proxy is intentionally dumb — header passthrough, body passthrough,
 * SSE streaming preserved by piping `Response.body` through. We don't
 * need to understand MCP framing here; the browser-side MCP SDK speaks
 * the protocol end-to-end.
 */

import * as crypto from 'node:crypto';
import * as fs from 'node:fs/promises';
import * as path from 'node:path';

import type { BunPlugin } from 'bun';

// Lazy sass import. Resolved on the first `.scss` request rather than
// at server boot so a missing `bun install` surfaces as a clean 500
// error page (with the resolve message in the JS comment) instead of
// crashing the Bun process before it can serve `/main.js`.
type SassModule = typeof import('sass');
let sassMod: Promise<SassModule> | null = null;
function loadSass(): Promise<SassModule> {
	if (sassMod === null) {
		sassMod = import('sass').catch((e: unknown) => {
			sassMod = null;
			throw new Error(`sass package not installed — run \`bun install\` in the playground dir (${(e as Error).message})`);
		});
	}
	return sassMod;
}

const PORT = Number(process.env.PORT ?? 3000);
// `path.resolve` canonicalises slashes to the OS native form so the
// `startsWith` path-traversal guard below works on both Windows and
// POSIX without ad-hoc separator handling.
const SERVER_DIR = path.resolve(
  path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1')),
);
const CRATE_DIR = path.resolve(SERVER_DIR, '..');
const REPO_ROOT = path.resolve(CRATE_DIR, '..', '..');
const WORKSPACE = path.resolve(process.env.STANDARDOC_WORKSPACE ?? REPO_ROOT);
const ENDPOINT_FILE = path.join(WORKSPACE, '.standardoc', 'mcp.endpoint');

const STATIC_ROOTS: ReadonlyArray<{ urlPrefix: string; fsRoot: string }> = [
  { urlPrefix: '/pkg/', fsRoot: path.join(CRATE_DIR, 'pkg') },
  { urlPrefix: '/', fsRoot: SERVER_DIR },
];

const CONTENT_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.mjs': 'application/javascript; charset=utf-8',
  '.ts': 'application/typescript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.wasm': 'application/wasm',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
};

let cachedEndpoint: { url: string; mtimeMs: number } | null = null;

async function readDaemonEndpoint(): Promise<string> {
  const stat = await fs.stat(ENDPOINT_FILE).catch(() => null);
  if (stat === null) {
    throw new Error(
      `Standardoc daemon endpoint not found at ${ENDPOINT_FILE}. ` +
        `Make sure the VSCode extension (or \`standardoc mcp --http <port>\`) is running on the workspace.`,
    );
  }
  if (cachedEndpoint !== null && cachedEndpoint.mtimeMs === stat.mtimeMs) {
    return cachedEndpoint.url;
  }
  const raw = (await fs.readFile(ENDPOINT_FILE, 'utf8')).trim();
  if (raw.length === 0) {
    throw new Error(`${ENDPOINT_FILE} is empty — daemon is still booting?`);
  }
  cachedEndpoint = { url: raw, mtimeMs: stat.mtimeMs };
  console.log(`[playground] daemon endpoint: ${raw}`);
  return raw;
}

async function serveStatic(reqPath: string): Promise<Response | null> {
  const decoded = decodeURIComponent(reqPath);
  // Block path traversal (`../`) by rejecting any segment that doesn't
  // resolve to a child of the configured roots.
  for (const root of STATIC_ROOTS) {
    if (!decoded.startsWith(root.urlPrefix)) continue;
    const tail = decoded.slice(root.urlPrefix.length) || 'index.html';
    const abs = path.resolve(root.fsRoot, tail);
    if (!abs.startsWith(root.fsRoot)) {
      return new Response('forbidden', { status: 403 });
    }
    const stat = await fs.stat(abs).catch(() => null);
    if (stat === null || !stat.isFile()) continue;
    const data = await fs.readFile(abs);
    const ct = CONTENT_TYPES[path.extname(abs).toLowerCase()] ?? 'application/octet-stream';
    return new Response(data, {
      headers: {
        'Content-Type': ct,
        'Cache-Control': 'no-store',
      },
    });
  }
  return null;
}

async function proxyMcp(req: Request, urlPath: string): Promise<Response> {
  let base: string;
  try {
    base = await readDaemonEndpoint();
  } catch (e) {
    return new Response(`MCP proxy unavailable: ${(e as Error).message}`, { status: 502 });
  }
  // The daemon endpoint already includes its own /mcp path segment in
  // most rmcp configurations; the URL we read IS the full MCP base.
  // We strip our local `/mcp` prefix and append the remainder to that
  // base, so `/mcp/foo` becomes `<base>/foo` and `/mcp` alone stays as
  // `<base>`.
  const tail = urlPath.slice('/mcp'.length);
  const target = base.endsWith('/') && tail.startsWith('/')
    ? base + tail.slice(1)
    : !base.endsWith('/') && tail.length > 0 && !tail.startsWith('/')
      ? `${base}/${tail}`
      : base + tail;

  const forward = new Request(target, {
    method: req.method,
    headers: stripHopHeaders(req.headers),
    body: req.method === 'GET' || req.method === 'HEAD' ? undefined : req.body,
    redirect: 'manual',
    // @ts-expect-error duplex is required by Bun/Undici when streaming a body.
    duplex: 'half',
  });

  const upstream = await fetch(forward);

  // SSE long-lived streams need extra care. Chrome strictly enforces
  // the HTTP/1.1 chunked-encoding contract: every response declared
  // `Transfer-Encoding: chunked` MUST terminate with the `0\r\n\r\n`
  // final chunk. When we pipe `upstream.body` directly into the
  // outgoing `Response`, Bun.serve sometimes propagates the upstream
  // close without emitting that terminator — Firefox forgives it,
  // Chrome aborts with `ERR_INCOMPLETE_CHUNKED_ENCODING` and the MCP
  // SDK enters a reconnect spin.
  //
  // Two defences:
  //   1. Strip hop-by-hop response headers (`transfer-encoding`,
  //      `connection`, `keep-alive`) so Bun is free to pick the right
  //      transport-level framing for the downstream client connection.
  //   2. Wrap an SSE body in a `TransformStream`. Bun.serve then sees
  //      a fresh stream owned by us, generates its own chunked
  //      encoding, and emits the final terminator on close.
  const upstreamCt = upstream.headers.get('content-type') ?? '';
  const isSse = upstreamCt.includes('text/event-stream');

  let body: BodyInit | null = upstream.body;
  if (isSse && upstream.body !== null) {
    const ts = new TransformStream<Uint8Array, Uint8Array>();
    void upstream.body.pipeTo(ts.writable).catch((e: unknown) => {
      const msg = e instanceof Error ? e.message : String(e);
      console.warn(`[playground] mcp SSE upstream pipe ended: ${msg}`);
    });
    body = ts.readable;
  }

  return new Response(body, {
    status: upstream.status,
    statusText: upstream.statusText,
    headers: relayHeaders(upstream.headers),
  });
}

function stripHopHeaders(h: Headers): Headers {
  // Hop-by-hop headers and Origin are stripped — the daemon doesn't
  // need browser context, and Origin would just confuse a potential
  // CORS check upstream.
  const out = new Headers();
  for (const [k, v] of h.entries()) {
    const lk = k.toLowerCase();
    if (
      lk === 'host' ||
      lk === 'connection' ||
      lk === 'keep-alive' ||
      lk === 'proxy-authorization' ||
      lk === 'te' ||
      lk === 'trailer' ||
      lk === 'transfer-encoding' ||
      lk === 'upgrade' ||
      lk === 'origin' ||
      lk === 'referer'
    ) {
      continue;
    }
    out.set(k, v);
  }
  return out;
}

function relayHeaders(h: Headers): Headers {
  const out = new Headers();
  for (const [k, v] of h.entries()) {
    const lk = k.toLowerCase();
    // Hop-by-hop response headers (RFC 7230 §6.1) MUST NOT be relayed
    // by a proxy — Bun.serve manages framing for the downstream
    // connection, so leaving `transfer-encoding: chunked` from the
    // upstream would either double-frame or conflict with Bun's own
    // encoding (Chromium then flags `ERR_INCOMPLETE_CHUNKED_ENCODING`
    // on the SSE close). `content-encoding` and `content-length` are
    // stripped for the same reason — we re-stream, so any upstream
    // length declaration is wrong by the time it reaches the client.
    if (
      lk === 'content-encoding' ||
      lk === 'content-length' ||
      lk === 'transfer-encoding' ||
      lk === 'connection' ||
      lk === 'keep-alive'
    ) {
      continue;
    }
    out.set(k, v);
  }
  // Permissive CORS for the local playground only — same-origin is
  // already enforced by Bun listening on 127.0.0.1 by default.
  out.set('Access-Control-Allow-Origin', '*');
  out.set('Access-Control-Allow-Headers', '*');
  out.set('Access-Control-Allow-Methods', '*');
  return out;
}

/**
 * SCSS modules plugin for Bun.build. Two flavours of `.scss` imports:
 *
 *   - `*.module.scss` → compile with sass, hash every classname so the
 *     module is collision-safe across components, then emit a JS
 *     module that injects the transformed CSS via a `<style>` element
 *     at runtime and default-exports the classname map.
 *
 *   - `*.scss` (non-module) → compile with sass and inject as a global
 *     stylesheet. No classname rewriting. Default export is `''`.
 *
 * sass.compile resolves `@use` paths itself relative to the entry
 * file, so partials (`_tokens.scss`, `_mixins.scss`) work without
 * extra plugin plumbing.
 */
const scssClassRegex = /\.([A-Za-z_][\w-]*)/g;

function shortHash(input: string): string {
	return crypto.createHash('sha1').update(input).digest('hex').slice(0, 8);
}

function transformCssModule(css: string, sourceAbsPath: string): { css: string; classes: Record<string, string> } {
	const hash = shortHash(sourceAbsPath);
	const classes: Record<string, string> = {};
	const transformed = css.replace(scssClassRegex, (_match, name: string) => {
		const hashed = `${name}_${hash}`;
		classes[name] = hashed;
		return `.${hashed}`;
	});
	return { css: transformed, classes };
}

function buildStyleInjector(css: string, sourceAbsPath: string, exportObject: string): string {
	const id = `sd-scss-${shortHash(sourceAbsPath)}`;
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
				const { css, classes } = transformCssModule(result.css, abs);
				return {
					contents: buildStyleInjector(css, abs, JSON.stringify(classes)),
					loader: 'js',
				};
			}
			return {
				contents: buildStyleInjector(result.css, abs, JSON.stringify('')),
				loader: 'js',
			};
		});
	},
};

/**
 * Bundle `main.ts` (and anything it imports, including the
 * wasm-bindgen-generated `pkg/standardoc_graph_viz.js`) into a single
 * browser-ready ESM module. Done lazily on each /main.js request so
 * that editing main.ts only needs a browser refresh, no server
 * restart. Bun's transpiler is fast enough that this is a few ms
 * for the scale we're operating at.
 */
async function bundleMain(): Promise<Response> {
  const entry = path.join(SERVER_DIR, 'main.ts');
  const built = await Bun.build({
    entrypoints: [entry],
    target: 'browser',
    format: 'esm',
    minify: false,
    sourcemap: 'inline',
    plugins: [scssModulesPlugin],
    // The wasm-bindgen generated loader does its own dynamic
    // `instantiateStreaming` against an URL we pass at runtime — no
    // need for Bun to inline the binary.
  });
  if (!built.success) {
    const log = built.logs.map(l => l.message).join('\n');
    return new Response(`/* build failed:\n${log}\n*/`, {
      status: 500,
      headers: { 'Content-Type': 'application/javascript; charset=utf-8' },
    });
  }
  const output = built.outputs[0];
  if (output === undefined) {
    return new Response('/* build produced no output */', {
      status: 500,
      headers: { 'Content-Type': 'application/javascript; charset=utf-8' },
    });
  }
  const text = await output.text();
  return new Response(text, {
    headers: {
      'Content-Type': 'application/javascript; charset=utf-8',
      'Cache-Control': 'no-store',
    },
  });
}

const server = Bun.serve({
  port: PORT,
  async fetch(req) {
    const url = new URL(req.url);
    if (req.method === 'OPTIONS') {
      return new Response(null, {
        status: 204,
        headers: {
          'Access-Control-Allow-Origin': '*',
          'Access-Control-Allow-Methods': '*',
          'Access-Control-Allow-Headers': '*',
          'Access-Control-Max-Age': '86400',
        },
      });
    }
    if (url.pathname === '/mcp' || url.pathname.startsWith('/mcp/')) {
      return proxyMcp(req, url.pathname + url.search);
    }
    if (url.pathname === '/main.js') {
      return bundleMain();
    }
    const served = await serveStatic(url.pathname);
    if (served !== null) return served;
    return new Response('Not found', { status: 404 });
  },
});

console.log(`[playground] http://${server.hostname}:${server.port}`);
console.log(`[playground] workspace: ${WORKSPACE}`);
console.log(`[playground] endpoint file: ${ENDPOINT_FILE}`);
