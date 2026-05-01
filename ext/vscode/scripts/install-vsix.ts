/// <reference types="bun-types" />
import { Glob } from 'bun';
import { spawnSync } from 'node:child_process';
import * as path from 'node:path';

const extRoot = path.resolve(import.meta.dir, '..');
const glob = new Glob('*.vsix');

const candidates: string[] = [];
for await (const file of glob.scan({ cwd: extRoot })) {
  candidates.push(file);
}

if (candidates.length === 0) {
  console.error('No .vsix found in', extRoot, '— run `bun run package` first.');
  process.exit(1);
}

const stats = await Promise.all(
  candidates.map(async name => ({
    name,
    mtime: (await Bun.file(path.join(extRoot, name)).stat()).mtimeMs,
  })),
);
stats.sort((a, b) => b.mtime - a.mtime);
const latest = stats[0]!.name;
const fullPath = path.join(extRoot, latest);

console.log(`Installing ${latest}…`);
const result = spawnSync('code', ['--install-extension', fullPath, '--force'], {
  stdio: 'inherit',
  shell: true,
});

if (result.status !== 0) {
  console.error('`code --install-extension` failed. Make sure the VSCode CLI is on your PATH.');
}
process.exit(result.status ?? 1);
