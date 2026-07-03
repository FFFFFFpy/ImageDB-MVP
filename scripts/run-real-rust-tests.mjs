import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const repoRoot = process.cwd();
const defaultPgBin = join(repoRoot, '.local', 'db-tools', 'postgresql-18.4', 'pgsql', 'bin');
const env = { ...process.env };

if (!env.IMAGEDB_POSTGRES_BIN && existsSync(defaultPgBin)) {
  env.IMAGEDB_POSTGRES_BIN = defaultPgBin;
}

const args = [
  'test',
  '--manifest-path',
  'apps/desktop/src-tauri/Cargo.toml',
  '--features',
  'real-db-tests,fail-injection',
  '--',
  '--ignored',
  '--test-threads=1',
];

const result = spawnSync('cargo', args, {
  cwd: repoRoot,
  env,
  stdio: 'inherit',
  shell: process.platform === 'win32',
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 1);
