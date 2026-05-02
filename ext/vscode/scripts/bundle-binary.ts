/// <reference types="bun-types" />
import * as path from 'node:path';
import * as fs from 'node:fs';

const extRoot = path.resolve(import.meta.dir, '..');
const workspaceRoot = path.resolve(extRoot, '..', '..');

const exe = process.platform === 'win32' ? 'stdoc.exe' : 'stdoc';

const candidates = [
  path.join(workspaceRoot, 'target', 'release', exe),
  path.join(workspaceRoot, 'target', 'debug', exe),
];

const present = candidates.filter(p => fs.existsSync(p));
if (present.length === 0) {
  console.error('No stdoc binary found. Tried:');
  for (const c of candidates) console.error(`  ${c}`);
  console.error('Run `bun run dev:daemon` or `bun run dev:daemon:release` first.');
  process.exit(1);
}
present.sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
const source = present[0]!;

const targetDir = path.join(extRoot, 'dist', 'bin');
const target = path.join(targetDir, exe);

if (fs.existsSync(targetDir)) {
  for (const entry of fs.readdirSync(targetDir)) {
    fs.rmSync(path.join(targetDir, entry), { force: true });
  }
}
fs.mkdirSync(targetDir, { recursive: true });

try {
  fs.copyFileSync(source, target);
} catch (e) {
  const code = (e as NodeJS.ErrnoException).code;
  if (code === 'EBUSY' || code === 'EPERM') {
    console.error(`Cannot overwrite ${target} — daemon may be running.`);
    console.error('Stop the daemon (close dev host or kill the process) and retry.');
    process.exit(1);
  }
  throw e;
}

if (process.platform !== 'win32') {
  fs.chmodSync(target, 0o755);
}

console.log(`Copied ${source} → ${target}`);
