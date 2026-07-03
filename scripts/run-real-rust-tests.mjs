import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const repoRoot = process.cwd();
const defaultPgBin = join(repoRoot, '.local', 'db-tools', 'postgresql-18.4', 'pgsql', 'bin');
const env = { ...process.env };

if (!env.IMAGEDB_POSTGRES_BIN && existsSync(defaultPgBin)) {
  env.IMAGEDB_POSTGRES_BIN = defaultPgBin;
}

const cargoManifest = 'apps/desktop/src-tauri/Cargo.toml';
const cargoCommand = 'cargo';
const suites = [
  { name: 'scan persistence', filter: 'real_scan_' },
  { name: 'source snapshot verification', filter: 'real_snapshot_' },
  { name: 'review persistence', filter: 'real_review_' },
  { name: 'file transaction protocol', filter: 'real_protocol_' },
  { name: 'formal commit pipeline', filter: 'real_commit_full_pipeline' },
  { name: 'strict manifest validation', filter: 'manifest_validation_' },
  { name: 'run-state reconciliation', filter: 'real_reconcile_' },
  {
    name: 'fault injection recovery',
    filter: 'fail_injection_',
    features: 'real-db-tests,fail-injection',
  },
];

if (env.IMAGEDB_REAL_TEST_PGVECTOR_LIFECYCLE === '1') {
  suites.unshift({ name: 'managed PostgreSQL lifecycle', filter: 'real_pgvector_full_lifecycle' });
}

for (const suite of suites) {
  const features = suite.features ?? 'real-db-tests';
  const args = [
    'test',
    '--manifest-path',
    cargoManifest,
    '--features',
    features,
    '--lib',
    suite.filter,
    '--',
    '--ignored',
    '--test-threads=1',
  ];

  console.log(`\n[real-rust] ${suite.name}`);
  const result = spawnSync(cargoCommand, args, {
    cwd: repoRoot,
    env,
    stdio: 'inherit',
  });

  if (result.error) {
    console.error(result.error.message);
    process.exit(1);
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
