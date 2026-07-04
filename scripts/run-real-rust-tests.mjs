import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const repoRoot = process.cwd();
const defaultPgBin = join(repoRoot, '.local', 'db-tools', 'postgresql-18.4', 'pgsql', 'bin');
const env = { ...process.env };

if (!env.IMAGEDB_POSTGRES_BIN && existsSync(defaultPgBin)) {
  env.IMAGEDB_POSTGRES_BIN = defaultPgBin;
}

// Fail-fast: if no PostgreSQL runtime is available, the real-DB tests would
// either panic (most do now) or — for any that still skip — silently report
// green without exercising the chain. The closure plan forbids the latter,
// so refuse to run at all and tell the operator exactly what is missing.
if (!env.IMAGEDB_POSTGRES_BIN) {
  console.error(
    `[real-rust] ABORT: IMAGEDB_POSTGRES_BIN is not set and no packaged runtime was found at\n` +
      `  ${defaultPgBin}\n` +
      `Real-DB tests cannot run without a PostgreSQL 18.x runtime. Run one of:\n` +
      `  - node scripts/package-postgres-runtime.mjs   (build the packaged runtime)\n` +
      `  - set IMAGEDB_POSTGRES_BIN=<path-to-pgsql-bin>\n`,
  );
  process.exit(1);
} else {
  console.log(`[real-rust] using PostgreSQL runtime: ${env.IMAGEDB_POSTGRES_BIN}`);
}

const cargoManifest = 'apps/desktop/src-tauri/Cargo.toml';
const cargoCommand = 'cargo';
const suites = [
  { name: 'scan persistence', filter: 'real_scan_' },
  { name: 'source snapshot verification', filter: 'real_snapshot_' },
  { name: 'review persistence', filter: 'real_review_' },
  { name: 'external empty database init', filter: 'real_external_empty_database_' },
  { name: 'external unreachable fallback', filter: 'real_external_unreachable_fallback_' },
  { name: 'external existing database compatibility', filter: 'real_external_existing_database_' },
  { name: 'external postgres migration', filter: 'real_migrate_managed_to_external_' },
  { name: 'file transaction protocol', filter: 'real_protocol_' },
  { name: 'formal commit pipeline', filter: 'real_commit_full_pipeline' },
  {
    name: 'M9 public command main chain',
    filter: 'm9_public_command_main_chain_first_run_to_completed_import',
  },
  {
    name: 'M9 diagnostics export',
    filter: 'm9_diagnostics_export_redacts_secrets_and_image_content',
  },
  {
    name: 'M9 public recovery command path',
    filter: 'm9_public_recovery_',
    features: 'real-db-tests,fail-injection',
  },
  { name: 'strict manifest validation', filter: 'manifest_validation_' },
  { name: 'run-state reconciliation', filter: 'real_reconcile_' },
  {
    name: 'fault injection recovery',
    filter: 'fail_injection_',
    features: 'real-db-tests,fail-injection',
  },
  {
    name: 'cancellation + final recovery invariants',
    filter: 'cancellation_recovery_',
    features: 'real-db-tests,fail-injection',
  },
];

// The pgvector lifecycle test was previously gated behind an env var because
// its migration-version assertion was stale (asserted 0007 after migration
// 0008 landed). That assertion is now fixed, so it runs by default.
suites.unshift({ name: 'managed PostgreSQL lifecycle', filter: 'real_pgvector_full_lifecycle' });

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
