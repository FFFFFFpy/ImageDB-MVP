import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const tauriDir = join(repoRoot, 'apps', 'desktop', 'src-tauri');
const releaseExe = join(tauriDir, 'target', 'release', 'imagedb-desktop.exe');
const bundleDir = join(tauriDir, 'target', 'release', 'bundle');
const nsisDir = join(bundleDir, 'nsis');
const runtimeDir = join(tauriDir, 'binaries', 'windows-x86_64', 'postgres-runtime');
const tauriConfig = JSON.parse(readFileSync(join(tauriDir, 'tauri.conf.json'), 'utf8'));
const expectedInstallerName = `${tauriConfig.productName}_${tauriConfig.version}_x64-setup.exe`;

const requiredRuntime = [
  ['runtime-manifest.json'],
  ['bin', 'postgres.exe'],
  ['bin', 'pg_ctl.exe'],
  ['bin', 'initdb.exe'],
  ['bin', 'psql.exe'],
  ['bin', 'pg_dump.exe'],
  ['lib', 'vector.dll'],
  ['share', 'extension', 'vector.control'],
  ['share', 'extension', 'vector--0.8.3.sql'],
];

function assertFile(path, label) {
  if (!existsSync(path) || !statSync(path).isFile()) {
    throw new Error(`${label} missing: ${path}`);
  }
}

assertFile(releaseExe, 'release executable');
for (const parts of requiredRuntime) {
  assertFile(join(runtimeDir, ...parts), `packaged runtime ${parts.join('/')}`);
}

if (!existsSync(nsisDir)) {
  throw new Error(`NSIS bundle directory missing: ${nsisDir}`);
}
const installers = readdirSync(nsisDir)
  .filter((name) => name.toLowerCase().endsWith('.exe'))
  .map((name) => join(nsisDir, name))
  .filter((path) => statSync(path).size > 1024 * 1024);

if (installers.length !== 1 || basename(installers[0]) !== expectedInstallerName) {
  throw new Error(
    `Expected exactly one current NSIS installer '${expectedInstallerName}', found: ${
      installers.map((path) => basename(path)).join(', ') || '<none>'
    }. Remove stale bundles and rebuild.`,
  );
}
if (statSync(installers[0]).mtimeMs + 5_000 < statSync(releaseExe).mtimeMs) {
  throw new Error(`NSIS installer is older than the release executable: ${installers[0]}`);
}

console.log('[release-artifacts] release executable:', releaseExe);
for (const installer of installers) {
  console.log('[release-artifacts] installer:', installer);
}
console.log('[release-artifacts] runtime:', runtimeDir);
