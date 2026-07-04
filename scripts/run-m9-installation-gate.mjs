import { existsSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const installer = join(
  repoRoot,
  'apps',
  'desktop',
  'src-tauri',
  'target',
  'release',
  'bundle',
  'nsis',
  'ImageDB_0.1.0_x64-setup.exe',
);
const baseDir = join(repoRoot, '.local', 'm9-install-gate');
const installDir = join(baseDir, 'ImageDB');
const appDataDir = join(baseDir, 'app-data');
const runtimeDir = join(installDir, 'postgres-runtime');

function assertFile(path, label) {
  if (!existsSync(path) || !statSync(path).isFile()) {
    throw new Error(`${label} missing: ${path}`);
  }
}

function assertDir(path, label) {
  if (!existsSync(path) || !statSync(path).isDirectory()) {
    throw new Error(`${label} missing: ${path}`);
  }
}

function run(command, args, label) {
  console.log(`[install-gate] ${label}`);
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    stdio: 'inherit',
    env: process.env,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${label} failed with exit code ${result.status}`);
  }
}

function install(label) {
  run(installer, ['/S', `/D=${installDir}`], label);
  assertFile(findInstalledExe(), 'installed executable');
  assertDir(runtimeDir, 'installed PostgreSQL runtime');
  for (const parts of [
    ['bin', 'postgres.exe'],
    ['bin', 'pg_ctl.exe'],
    ['bin', 'initdb.exe'],
    ['bin', 'psql.exe'],
    ['bin', 'pg_dump.exe'],
    ['lib', 'vector.dll'],
    ['share', 'extension', 'vector.control'],
    ['share', 'extension', 'vector--0.8.3.sql'],
  ]) {
    assertFile(join(runtimeDir, ...parts), `installed runtime ${parts.join('/')}`);
  }
}

async function launchSmoke() {
  console.log('[install-gate] launch installed executable');
  mkdirSync(appDataDir, { recursive: true });
  const child = spawn(findInstalledExe(), {
    cwd: installDir,
    env: {
      ...process.env,
      IMAGEDB_APP_DATA_DIR: appDataDir,
    },
    stdio: 'ignore',
    windowsHide: true,
  });

  await new Promise((resolve) => setTimeout(resolve, 5000));
  if (child.exitCode !== null) {
    throw new Error(`installed executable exited early with code ${child.exitCode}`);
  }
  spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], {
    stdio: 'ignore',
  });
  await new Promise((resolve) => child.once('exit', resolve));
  console.log('[install-gate] installed executable stayed alive for smoke window');
}

function findUninstaller() {
  const candidates = readdirSync(installDir)
    .filter((name) => name.toLowerCase().endsWith('.exe'))
    .filter((name) => name.toLowerCase().includes('uninstall'));
  if (candidates.length === 0) {
    throw new Error(`uninstaller not found in ${installDir}`);
  }
  return join(installDir, candidates[0]);
}

function findInstalledExe() {
  const candidates = readdirSync(installDir)
    .filter((name) => name.toLowerCase().endsWith('.exe'))
    .filter((name) => !name.toLowerCase().includes('uninstall'));
  if (candidates.length === 0) {
    throw new Error(`installed executable not found in ${installDir}`);
  }
  return join(installDir, candidates[0]);
}

async function uninstallAndVerifyRetention() {
  const sentinel = join(appDataDir, 'data-retention-sentinel.txt');
  writeFileSync(sentinel, 'must survive uninstall\n');
  const uninstaller = findUninstaller();
  run(uninstaller, ['/S'], 'silent uninstall');
  await waitForUninstall();
  assertFile(sentinel, 'retained app data sentinel');
}

async function waitForUninstall() {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    if (!hasInstalledMainExe()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  if (existsSync(installDir)) {
    const remainingExe = readdirSync(installDir)
      .filter((name) => name.toLowerCase().endsWith('.exe'))
      .filter((name) => !name.toLowerCase().includes('uninstall'));
    if (remainingExe.length > 0) {
      throw new Error(`installed executable still exists after uninstall: ${remainingExe[0]}`);
    }
  }
}

function hasInstalledMainExe() {
  if (existsSync(installDir)) {
    const remainingExe = readdirSync(installDir)
      .filter((name) => name.toLowerCase().endsWith('.exe'))
      .filter((name) => !name.toLowerCase().includes('uninstall'));
    if (remainingExe.length > 0) {
      return true;
    }
  }
  return false;
}

async function main() {
  if (process.platform !== 'win32') {
    throw new Error(
      'install-gate requires Windows and the NSIS installer; run it on a clean Windows machine.',
    );
  }

  assertFile(installer, 'NSIS installer');
  rmSync(baseDir, { recursive: true, force: true });
  mkdirSync(baseDir, { recursive: true });

  install('silent install');
  await launchSmoke();
  install('same-version overwrite install');
  await uninstallAndVerifyRetention();

  console.log('[install-gate] passed');
  console.log(`[install-gate] install dir: ${installDir}`);
  console.log(`[install-gate] retained app data: ${appDataDir}`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
