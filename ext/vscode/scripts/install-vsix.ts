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

const codeCmd = process.platform === 'win32' ? 'code.cmd' : 'code';

console.log(`Installing ${latest} via ${codeCmd}…`);
const result = spawnSync(codeCmd, ['--install-extension', fullPath, '--force'], {
  stdio: 'inherit',
  shell: true,
});

if (result.status === 0) {
  console.log(`Installed ${latest}.`);
} else {
  console.error(`\`${codeCmd} --install-extension\` failed (status ${result.status}).`);
  if (process.platform === 'win32') {
    console.error('On Windows, ensure `<VSCode install>\\bin` is on your PATH so');
    console.error('that `code.cmd` resolves to the CLI launcher (not `Code.exe`, the GUI).');
  }
  console.error(`Fallback: VSCode → Ctrl+Shift+X → ... → Install from VSIX → ${fullPath}`);
}
process.exit(result.status ?? 1);
