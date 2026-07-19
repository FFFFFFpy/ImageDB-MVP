import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';

const repoRoot = fileURLToPath(new URL('..', import.meta.url));
const desktopDir = join(repoRoot, 'apps', 'desktop');

function git(args) {
  return execFileSync('git', ['-C', repoRoot, ...args], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
  }).trim();
}

const commit = process.env.IMAGEDB_GIT_COMMIT?.trim() || git(['rev-parse', 'HEAD']);
const dirty =
  process.env.IMAGEDB_GIT_DIRTY?.trim() ||
  String(git(['status', '--porcelain', '--untracked-files=no']).length > 0);
const env = {
  ...process.env,
  IMAGEDB_GIT_COMMIT: commit,
  IMAGEDB_GIT_DIRTY: dirty,
};

const command = process.platform === 'win32' ? (process.env.ComSpec ?? 'cmd.exe') : 'pnpm';
const args =
  process.platform === 'win32'
    ? ['/d', '/s', '/c', 'pnpm exec tauri build --features bundled-runtime-required']
    : ['exec', 'tauri', 'build', '--features', 'bundled-runtime-required'];

console.log(`[tauri-build] commit=${commit} tracked_dirty=${dirty}`);
const result = spawnSync(command, args, {
  cwd: desktopDir,
  env,
  stdio: 'inherit',
});
if (result.error) {
  throw result.error;
}
process.exit(result.status ?? 1);
