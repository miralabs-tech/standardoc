// Build pipeline for the Standardoc VSCode extension. Produces two
// bundles:
//   - host:    src/extension.ts  -> dist/extension.js   (node, cjs)
//   - webview: src/webview/main.ts -> dist/webview/shell.js (browser, esm)
//
// The webview bundle pulls in the viz lib (`@standarx/standardoc-viz`,
// resolved via tsconfig paths) whose components import `.scss` /
// `.module.scss`; those are turned into runtime <style> injectors by
// the SCSS plugin below (ported from the playground dev server). The
// host bundle has no SCSS, so the plugin is webview-only.
//
// The wasm binary (`standardoc_graph_viz_bg.wasm`) is copied next to
// the webview bundle; the host injects an asWebviewUri to it and the
// wasm-bindgen `--target web` loader fetches it by URL at runtime.

import * as path from 'node:path';
import * as fs from 'node:fs/promises';
import * as crypto from 'node:crypto';
import type { BunPlugin } from 'bun';

const production = process.argv.includes('--production');

const ROOT = import.meta.dir;
// Walk up with dirname rather than `path.resolve(ROOT, '..', '..')` —
// the latter silently drops the `..` segments under Bun on Windows.
// ext/vscode -> ext -> <repo root>.
const REPO_ROOT = path.dirname(path.dirname(ROOT));
const VIZ_CRATE = path.join(REPO_ROOT, 'crates', 'standardoc-graph-viz');
const PKG_DIR = path.join(VIZ_CRATE, 'pkg');
const PKG_JS = path.join(PKG_DIR, 'standardoc_graph_viz.js');
const PKG_WASM = path.join(PKG_DIR, 'standardoc_graph_viz_bg.wasm');
const DIST = path.join(ROOT, 'dist');
const WEBVIEW_DIST = path.join(DIST, 'webview');

// --- SCSS plugin (compile + runtime <style> injector) ---
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

async function exists(p: string): Promise<boolean> {
  try {
    await fs.access(p);
    return true;
  } catch {
    return false;
  }
}

async function ensureWasm(): Promise<void> {
  if (await exists(PKG_JS)) return;
  console.log('[build] pkg/ missing — running wasm-pack build…');
  const proc = Bun.spawnSync(
    ['wasm-pack', 'build', VIZ_CRATE, '--target', 'web', '--out-dir', 'pkg', ...(production ? [] : ['--dev'])],
    { stdout: 'inherit', stderr: 'inherit' },
  );
  if (proc.exitCode !== 0) {
    throw new Error(`wasm-pack build failed (exit ${proc.exitCode}). Install via \`cargo install wasm-pack\`.`);
  }
}

async function buildHost(): Promise<void> {
  const r = await Bun.build({
    entrypoints: [path.join(ROOT, 'src', 'extension.ts')],
    target: 'node',
    format: 'cjs',
    outdir: DIST,
    external: ['vscode'],
    minify: production,
    sourcemap: production ? 'none' : 'external',
  });
  if (!r.success) {
    throw new Error('host bundle failed:\n' + r.logs.map(l => l.message).join('\n'));
  }
  console.log('[build] host -> dist/extension.js');
}

async function buildWebview(): Promise<void> {
  await fs.mkdir(WEBVIEW_DIST, { recursive: true });
  const r = await Bun.build({
    entrypoints: [path.join(ROOT, 'src', 'webview', 'main.ts')],
    target: 'browser',
    format: 'esm',
    minify: production,
    sourcemap: production ? 'none' : 'inline',
    plugins: [scssModulesPlugin],
  });
  if (!r.success) {
    throw new Error('webview bundle failed:\n' + r.logs.map(l => l.message).join('\n'));
  }
  const out = r.outputs[0];
  if (out === undefined) throw new Error('webview bundle produced no output');
  await Bun.write(path.join(WEBVIEW_DIST, 'shell.js'), await out.text());
  await fs.copyFile(PKG_WASM, path.join(WEBVIEW_DIST, 'standardoc_graph_viz_bg.wasm'));
  console.log('[build] webview -> dist/webview/shell.js (+ wasm)');
}

async function main(): Promise<void> {
  await ensureWasm();
  await Promise.all([buildHost(), buildWebview()]);
  console.log('[build] done.');
}

main().catch(err => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
