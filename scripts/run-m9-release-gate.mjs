import { existsSync } from 'node:fs';
import { join, parse } from 'node:path';
import { spawnSync } from 'node:child_process';

const repoRoot = process.cwd();
const defaultPgBin = join(repoRoot, '.local', 'db-tools', 'postgresql-18.4', 'pgsql', 'bin');
const baseEnv = { ...process.env };
const postgresBin =
  baseEnv.IMAGEDB_POSTGRES_BIN || (existsSync(defaultPgBin) ? defaultPgBin : null);

const args = process.argv.slice(2);
const onlyArg = args.find((arg) => arg.startsWith('--only='));
const only = onlyArg ? onlyArg.slice('--only='.length) : null;
const skipInstall = args.includes('--skip-install');
const skipBuild = args.includes('--skip-build');
const skipReal = args.includes('--skip-real');
const skipMounted = args.includes('--skip-mounted');
const noLoopbackSmb = args.includes('--no-loopback-smb');

const steps = [
  { id: 'install', label: 'pnpm install', command: 'pnpm', args: ['install'], skip: skipInstall },
  {
    id: 'format',
    label: 'pnpm format:check',
    command: 'pnpm',
    args: ['format:check'],
  },
  { id: 'typecheck', label: 'pnpm typecheck', command: 'pnpm', args: ['typecheck'] },
  { id: 'unit', label: 'pnpm test:unit', command: 'pnpm', args: ['test:unit'] },
  {
    id: 'rust',
    label: 'pnpm rust:test',
    command: 'pnpm',
    args: ['rust:test'],
    cleanPostgresEnv: true,
  },
  { id: 'clippy', label: 'pnpm rust:clippy', command: 'pnpm', args: ['rust:clippy'] },
  {
    id: 'real',
    label: 'pnpm rust:test:real',
    command: 'pnpm',
    args: ['rust:test:real'],
    skip: skipReal,
  },
  {
    id: 'performance',
    label: 'pnpm release:performance',
    command: 'pnpm',
    args: ['release:performance'],
  },
  { id: 'mounted', label: 'mounted SMB storage gate', mounted: true, skip: skipMounted },
  { id: 'build', label: 'pnpm build', command: 'pnpm', args: ['build'], skip: skipBuild },
  {
    id: 'verify-artifacts',
    label: 'pnpm release:verify-artifacts',
    command: 'pnpm',
    args: ['release:verify-artifacts'],
    skip: skipBuild,
  },
];

function shouldRun(step) {
  return (!only || step.id === only) && !step.skip;
}

function runStep(step) {
  console.log(`\n[m9-release-gate] ${step.label}`);
  const started = Date.now();
  const { command, args } = resolveCommand(step.command, step.args);
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    env: envForStep(step),
    stdio: 'inherit',
  });
  const seconds = ((Date.now() - started) / 1000).toFixed(1);

  if (result.error) {
    console.error(`[m9-release-gate] ${step.label} failed to start: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`[m9-release-gate] ${step.label} failed after ${seconds}s`);
    process.exit(result.status ?? 1);
  }
  console.log(`[m9-release-gate] ${step.label} passed in ${seconds}s`);
}

function envForStep(step) {
  const stepEnv = { ...baseEnv };
  if (step.cleanPostgresEnv) {
    delete stepEnv.IMAGEDB_POSTGRES_BIN;
    delete stepEnv.IMAGEDB_POSTGRES_RUNTIME_DIR;
  }
  if (step.needsPostgresBin && postgresBin) {
    stepEnv.IMAGEDB_POSTGRES_BIN = postgresBin;
  }
  return stepEnv;
}

function resolveCommand(command, args) {
  if (process.platform === 'win32' && command === 'pnpm') {
    return {
      command: process.env.ComSpec ?? 'cmd.exe',
      args: ['/d', '/s', '/c', ['pnpm', ...args].join(' ')],
    };
  }
  return { command, args };
}

function runMountedGate() {
  if (!postgresBin) {
    console.error('IMAGEDB_POSTGRES_BIN is required for the mounted storage gate');
    process.exit(1);
  }

  if (baseEnv.IMAGEDB_MOUNTED_LIBRARY_ROOT) {
    runStep({
      label: 'mounted storage gate from environment',
      command: 'cargo',
      args: [
        'test',
        '--manifest-path',
        'apps/desktop/src-tauri/Cargo.toml',
        '--features',
        'fail-injection,real-db-tests',
        '--lib',
        'mounted_storage_gate_library_root_disconnect_pauses_then_recovers',
        '--',
        '--ignored',
        '--test-threads=1',
      ],
      needsPostgresBin: true,
    });
    return;
  }

  if (process.platform !== 'win32' || noLoopbackSmb) {
    console.error(
      'Set IMAGEDB_MOUNTED_LIBRARY_ROOT, or run on Windows without --no-loopback-smb to use the loopback SMB gate.',
    );
    process.exit(1);
  }

  const repoRootInfo = parse(repoRoot);
  const driveName = repoRootInfo.root.replace(/[\\/]+$/, '').replace(/:$/, '');
  const remotePath = `\\\\localhost\\${driveName}$`;
  const repoRelative = repoRoot.slice(repoRootInfo.root.length);
  const escapedRepoRelative = repoRelative.replace(/'/g, "''");
  const escapedRemotePath = remotePath.replace(/'/g, "''");

  const script = `
$ErrorActionPreference = 'Stop'
$remote = '${escapedRemotePath}'
if (-not (Test-Path -LiteralPath $remote)) { throw "SMB remote path not accessible: $remote" }
$drive = $null
foreach ($candidate in 'Z','Y','X','W','V','U','T','S','R','Q','P') {
  $candidateDrive = $candidate + ':'
  if (-not (Test-Path $candidateDrive)) { $drive = $candidateDrive; break }
}
if (-not $drive) { throw 'No free drive letter for SMB mapping test' }
$runId = [guid]::NewGuid().ToString('N')
$mappingCreated = $false
try {
  New-SmbMapping -LocalPath $drive -RemotePath $remote -Persistent $false | Out-Null
  $mappingCreated = $true
  $repoViaMapping = Join-Path ($drive + '\\') '${escapedRepoRelative}'
  $mountedBase = Join-Path $repoViaMapping ".local\\m9-smb-admin-$runId"
  New-Item -ItemType Directory -Force -Path $mountedBase | Out-Null
  $env:IMAGEDB_MOUNTED_LIBRARY_ROOT = $mountedBase
  $env:IMAGEDB_MOUNTED_LOCAL_PATH = $drive
  $env:IMAGEDB_MOUNTED_REMOTE_PATH = $remote
  cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features fail-injection,real-db-tests --lib mounted_storage_gate_library_root_disconnect_pauses_then_recovers -- --ignored --test-threads=1
}
finally {
  if ($mappingCreated) {
    Remove-SmbMapping -LocalPath $drive -Force -UpdateProfile:$false -ErrorAction SilentlyContinue
  }
  $localCleanup = Join-Path '${repoRoot.replace(/'/g, "''")}' ".local\\m9-smb-admin-$runId"
  Remove-Item -LiteralPath $localCleanup -Recurse -Force -ErrorAction SilentlyContinue
}
`;

  runStep({
    label: 'mounted storage gate using Windows loopback SMB',
    command: 'powershell',
    args: ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', script],
    needsPostgresBin: true,
  });
}

const selectedSteps = steps.filter(shouldRun);
if (selectedSteps.length === 0) {
  console.error(`No release gate steps selected${only ? ` for --only=${only}` : ''}.`);
  process.exit(1);
}

for (const step of selectedSteps) {
  if (step.mounted) {
    console.log(`\n[m9-release-gate] ${step.label}`);
    const started = Date.now();
    runMountedGate();
    const seconds = ((Date.now() - started) / 1000).toFixed(1);
    console.log(`[m9-release-gate] ${step.label} passed in ${seconds}s`);
  } else {
    runStep(step);
  }
}
