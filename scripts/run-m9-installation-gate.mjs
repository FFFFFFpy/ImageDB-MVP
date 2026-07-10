import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmdirSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:net';

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const tauriConfig = JSON.parse(
  readFileSync(join(repoRoot, 'apps', 'desktop', 'src-tauri', 'tauri.conf.json'), 'utf8'),
);
const installer = join(
  repoRoot,
  'apps',
  'desktop',
  'src-tauri',
  'target',
  'release',
  'bundle',
  'nsis',
  `${tauriConfig.productName}_${tauriConfig.version}_x64-setup.exe`,
);
const baseDir = join(repoRoot, '.local', 'm9-install-gate');
const installDir = join(baseDir, 'ImageDB');
const appDataDir = join(baseDir, 'app-data');
const runtimeDir = join(installDir, 'postgres-runtime');
const managedPostgresDataDir = join(appDataDir, 'postgres_data');
const defaultAppDataDir = join(process.env.LOCALAPPDATA ?? '', 'ImageDB');
const defaultAppDataExistedBeforeGate = existsSync(defaultAppDataDir);

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

function comparablePath(path) {
  let normalized = path;
  if (normalized.startsWith('\\\\?\\UNC\\')) {
    normalized = `\\\\${normalized.slice(8)}`;
  } else if (normalized.startsWith('\\\\?\\')) {
    normalized = normalized.slice(4);
  }
  return resolve(normalized)
    .replace(/[\\/]+$/, '')
    .toLowerCase();
}

function assertSamePath(actual, expected, label) {
  if (comparablePath(actual) !== comparablePath(expected)) {
    throw new Error(`${label} mismatch: expected ${expected}, got ${actual}`);
  }
}

function run(command, args, label, env = process.env) {
  console.log(`[install-gate] ${label}`);
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    stdio: 'inherit',
    env,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${label} failed with exit code ${result.status}`);
  }
}

function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : null;
      server.close((error) => {
        if (error) reject(error);
        else if (port === null) reject(new Error('failed to allocate PostgreSQL smoke port'));
        else resolve(port);
      });
    });
  });
}

async function verifyInstalledRuntime() {
  console.log('[install-gate] initialize installed PostgreSQL runtime');
  const dataDir = join(baseDir, 'runtime smoke 数据', 'postgres data');
  const logFile = join(baseDir, 'runtime smoke 数据', 'postgres.log');
  mkdirSync(dirname(dataDir), { recursive: true });
  const port = await getFreePort();
  const binDir = join(runtimeDir, 'bin');
  const env = { ...process.env, PATH: `${binDir};${process.env.PATH ?? ''}` };
  const initdb = join(binDir, 'initdb.exe');
  const pgCtl = join(binDir, 'pg_ctl.exe');
  const psql = join(binDir, 'psql.exe');

  run(
    initdb,
    ['-D', dataDir, '--no-locale', '--encoding=UTF8', '--auth=trust', '--username=imagedb'],
    'installed runtime initdb',
    env,
  );

  let started = false;
  try {
    run(
      pgCtl,
      ['start', '-D', dataDir, '-l', logFile, '-o', `-p ${port} -h 127.0.0.1`, '-w', '-t', '45'],
      'installed runtime pg_ctl start',
      env,
    );
    started = true;
    run(
      psql,
      [
        '-h',
        '127.0.0.1',
        '-p',
        String(port),
        '-U',
        'imagedb',
        '-d',
        'postgres',
        '-c',
        'CREATE DATABASE imagedb',
      ],
      'installed runtime create database',
      env,
    );
    run(
      psql,
      [
        '-h',
        '127.0.0.1',
        '-p',
        String(port),
        '-U',
        'imagedb',
        '-d',
        'imagedb',
        '-c',
        "CREATE EXTENSION vector; SELECT extversion FROM pg_extension WHERE extname = 'vector'",
      ],
      'installed runtime pgvector smoke',
      env,
    );
  } finally {
    if (started) {
      run(pgCtl, ['stop', '-D', dataDir, '-m', 'fast', '-w'], 'installed runtime pg_ctl stop', env);
    }
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

function assertManagedPostgresStopped(label) {
  const pgCtl = join(runtimeDir, 'bin', 'pg_ctl.exe');
  assertFile(pgCtl, 'installed runtime pg_ctl');
  assertDir(managedPostgresDataDir, 'managed PostgreSQL data directory');
  const binDir = join(runtimeDir, 'bin');
  const result = spawnSync(pgCtl, ['status', '-D', managedPostgresDataDir], {
    cwd: repoRoot,
    encoding: 'utf8',
    env: { ...process.env, PATH: `${binDir};${process.env.PATH ?? ''}` },
    windowsHide: true,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status === 0) {
    throw new Error(`${label}: managed PostgreSQL is still running`);
  }
  // PostgreSQL's documented pg_ctl status code for "no server running" is 3.
  // Other non-zero codes indicate invalid data/runtime state and must not be
  // accepted as a successful shutdown.
  if (result.status !== 3) {
    throw new Error(
      `${label}: unexpected pg_ctl status ${result.status}\n${result.stdout ?? ''}${result.stderr ?? ''}`,
    );
  }
  if (existsSync(join(managedPostgresDataDir, 'postmaster.pid'))) {
    throw new Error(`${label}: stale postmaster.pid remains after shutdown`);
  }
  console.log(`[install-gate] ${label}: managed PostgreSQL is stopped`);
}

async function launchSmoke() {
  console.log('[install-gate] launch installed executable');
  mkdirSync(appDataDir, { recursive: true });
  const child = spawn(findInstalledExe(), ['--imagedb-install-gate-launch-smoke'], {
    cwd: installDir,
    env: {
      ...process.env,
      IMAGEDB_APP_DATA_DIR: appDataDir,
    },
    stdio: 'inherit',
    windowsHide: true,
  });

  let launchError = null;
  let observedManagedPostgres = false;
  child.once('error', (error) => {
    launchError = error;
  });
  const deadline = Date.now() + 30000;
  while (
    child.exitCode === null &&
    child.signalCode === null &&
    launchError === null &&
    Date.now() < deadline
  ) {
    if (existsSync(join(managedPostgresDataDir, 'postmaster.pid'))) {
      observedManagedPostgres = true;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  if (launchError) {
    throw launchError;
  }
  if (child.exitCode === null && child.signalCode === null) {
    child.kill();
    throw new Error('installed launch smoke did not exit gracefully within 30 seconds');
  }
  if (child.signalCode !== null) {
    throw new Error(`installed launch smoke was terminated by signal ${child.signalCode}`);
  }
  if (child.exitCode !== 0) {
    throw new Error(`installed launch smoke exited with code ${child.exitCode}`);
  }
  if (!observedManagedPostgres) {
    throw new Error('installed launch smoke never started the managed PostgreSQL cluster');
  }
  assertManagedPostgresStopped('graceful launch smoke exit');
  console.log('[install-gate] installed executable completed graceful launch smoke');
}

function verifyInstalledApplicationBootstrap() {
  const env = {
    ...process.env,
    IMAGEDB_APP_DATA_DIR: appDataDir,
    // Deliberately poison every runtime override. The installed app probe
    // only passes if normal Tauri setup replaces these with its resource-dir
    // runtime and enables the strict bundled-runtime policy.
    IMAGEDB_POSTGRES_RUNTIME_DIR: join(baseDir, 'poisoned-runtime'),
    IMAGEDB_POSTGRES_BIN: join(baseDir, 'poisoned-bin'),
    IMAGEDB_POSTGRES_RUNTIME_REQUIRED: '0',
  };
  const label = 'installed application managed bootstrap probe';
  console.log(`[install-gate] ${label}`);
  const result = spawnSync(findInstalledExe(), ['--imagedb-install-gate-managed-bootstrap'], {
    cwd: repoRoot,
    encoding: 'utf8',
    env,
    windowsHide: true,
  });
  if (result.stdout) process.stdout.write(result.stdout);
  if (result.stderr) process.stderr.write(result.stderr);
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${label} failed with exit code ${result.status}`);
  }

  const prefix = 'IMAGEDB_INSTALL_PROBE_JSON=';
  const jsonLine = (result.stdout ?? '').split(/\r?\n/).find((line) => line.startsWith(prefix));
  if (!jsonLine) {
    throw new Error(`${label} did not emit ${prefix}<json>`);
  }
  const probe = JSON.parse(jsonLine.slice(prefix.length));
  if (probe.status !== 'passed') {
    throw new Error(`${label} reported unexpected status: ${probe.status}`);
  }
  assertSamePath(probe.resource_dir, installDir, 'Tauri resource directory');
  assertSamePath(probe.runtime_dir, runtimeDir, 'strict bundled runtime directory');
  if (probe.runtime_required !== '1') {
    throw new Error(`${label} did not enable strict runtime policy`);
  }
  if (probe.postgres_bin_cleared !== true) {
    throw new Error(`${label} did not clear the inherited PostgreSQL bin override`);
  }
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
  let defaultSentinel = null;
  if (defaultAppDataExistedBeforeGate) {
    console.log(
      `[install-gate] default app-data retention probe skipped to avoid modifying existing user data: ${defaultAppDataDir}`,
    );
  } else {
    mkdirSync(defaultAppDataDir, { recursive: true });
    defaultSentinel = join(defaultAppDataDir, 'imagedb-install-gate-retention-sentinel.txt');
    writeFileSync(defaultSentinel, 'default app data must survive uninstall\n');
  }

  const uninstaller = findUninstaller();
  run(uninstaller, ['/S'], 'silent uninstall');
  await waitForUninstall();
  assertFile(sentinel, 'retained app data sentinel');
  if (defaultSentinel !== null) {
    assertFile(defaultSentinel, 'retained default app data sentinel');
    unlinkSync(defaultSentinel);
    rmdirSync(defaultAppDataDir);
    console.log('[install-gate] default app-data retention verified');
  }
}

async function waitForUninstall() {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    if (!existsSync(installDir)) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  if (existsSync(installDir)) {
    const remaining = readdirSync(installDir).join(', ') || '<empty directory>';
    throw new Error(`install directory still exists after uninstall: ${installDir} (${remaining})`);
  }
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
  await verifyInstalledRuntime();
  verifyInstalledApplicationBootstrap();
  assertManagedPostgresStopped('managed bootstrap probe exit');
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
