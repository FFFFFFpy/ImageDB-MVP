import { cpSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const pgRoot = join(repoRoot, '.local', 'db-tools', 'postgresql-18.4', 'pgsql');
const vectorRoot = join(repoRoot, '.local', 'db-tools', 'pgvector-0.8.3-pg18');
const outRoot = join(
  repoRoot,
  'apps',
  'desktop',
  'src-tauri',
  'binaries',
  'windows-x86_64',
  'postgres-runtime',
);

const requiredPgFiles = [
  ['bin', 'postgres.exe'],
  ['bin', 'pg_ctl.exe'],
  ['bin', 'initdb.exe'],
  ['bin', 'psql.exe'],
  ['bin', 'pg_dump.exe'],
  ['lib'],
  ['share'],
];

const requiredVectorFiles = [
  ['lib', 'vector.dll'],
  ['share', 'extension', 'vector.control'],
  ['share', 'extension', 'vector--0.8.3.sql'],
];

function requirePath(root, parts, label) {
  const path = join(root, ...parts);
  if (!existsSync(path)) {
    throw new Error(`${label} missing: ${path}`);
  }
  return path;
}

for (const parts of requiredPgFiles) {
  requirePath(pgRoot, parts, 'PostgreSQL runtime');
}
for (const parts of requiredVectorFiles) {
  requirePath(vectorRoot, parts, 'pgvector runtime');
}

rmSync(outRoot, { recursive: true, force: true });
mkdirSync(outRoot, { recursive: true });

for (const dir of ['bin', 'lib', 'share']) {
  cpSync(join(pgRoot, dir), join(outRoot, dir), { recursive: true });
}

cpSync(join(vectorRoot, 'lib', 'vector.dll'), join(outRoot, 'lib', 'vector.dll'));
cpSync(join(vectorRoot, 'share', 'extension'), join(outRoot, 'share', 'extension'), {
  recursive: true,
});

writeFileSync(
  join(outRoot, 'runtime-manifest.json'),
  `${JSON.stringify(
    {
      schema_version: 1,
      postgres_version: '18.4',
      pgvector_version: '0.8.3-pg18',
      layout: 'postgres-runtime',
      required_binaries: ['postgres.exe', 'pg_ctl.exe', 'initdb.exe', 'psql.exe', 'pg_dump.exe'],
    },
    null,
    2,
  )}\n`,
);

const notices = [
  'ImageDB bundled PostgreSQL runtime',
  '',
  'PostgreSQL files are copied from .local/db-tools/postgresql-18.4/pgsql.',
  'pgvector files are copied from .local/db-tools/pgvector-0.8.3-pg18.',
  'See the source distributions and bundled license files for third-party notices.',
  '',
];
writeFileSync(join(outRoot, 'THIRD_PARTY_NOTICES.txt'), notices.join('\n'));

console.log(`[runtime] packaged PostgreSQL runtime: ${outRoot}`);
