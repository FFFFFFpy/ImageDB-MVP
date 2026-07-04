import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const repoRoot = process.cwd();
const defaultPgBin = join(repoRoot, '.local', 'db-tools', 'postgresql-18.4', 'pgsql', 'bin');
const reportPath = join(repoRoot, 'reports', 'm9-performance-thresholds.json');
const imageCount = Number.parseInt(process.env.IMAGEDB_M9_PERF_IMAGE_COUNT ?? '120', 10);
const env = { ...process.env, IMAGEDB_M9_PERF_IMAGE_COUNT: String(imageCount) };

if (!env.IMAGEDB_POSTGRES_BIN && existsSync(defaultPgBin)) {
  env.IMAGEDB_POSTGRES_BIN = defaultPgBin;
}

const thresholds = {
  image_count_min: 120,
  managed_startup_ms_max: 15000,
  scan_ms_max: 60000,
  scan_images_per_sec_min: 5,
  plan_ms_max: 15000,
  commit_ms_max: 60000,
  commit_images_per_sec_min: 5,
  recovery_scan_empty_ms_max: 5000,
  total_ms_max: 120000,
};

const result = spawnSync(
  'cargo',
  [
    'test',
    '--manifest-path',
    'apps/desktop/src-tauri/Cargo.toml',
    '--features',
    'real-db-tests',
    '--lib',
    'm9_performance_gate_records_thresholds',
    '--',
    '--ignored',
    '--test-threads=1',
    '--nocapture',
  ],
  {
    cwd: repoRoot,
    env,
    encoding: 'utf8',
  },
);

process.stdout.write(result.stdout ?? '');
process.stderr.write(result.stderr ?? '');

if (result.error) {
  throw result.error;
}
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const combined = `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
const match = combined.match(/M9_PERFORMANCE_METRICS_JSON=(\{.*\})/);
if (!match) {
  console.error('[m9-performance] metrics marker not found in cargo output');
  process.exit(1);
}

const metrics = JSON.parse(match[1]);
const failures = [];

function check(name, ok, detail) {
  if (!ok) failures.push(`${name}: ${detail}`);
}

check(
  'image_count_min',
  metrics.image_count >= thresholds.image_count_min,
  `${metrics.image_count} < ${thresholds.image_count_min}`,
);
check(
  'managed_startup_ms_max',
  metrics.managed_startup_ms <= thresholds.managed_startup_ms_max,
  `${metrics.managed_startup_ms} > ${thresholds.managed_startup_ms_max}`,
);
check(
  'scan_ms_max',
  metrics.scan_ms <= thresholds.scan_ms_max,
  `${metrics.scan_ms} > ${thresholds.scan_ms_max}`,
);
check(
  'scan_images_per_sec_min',
  metrics.scan_images_per_sec >= thresholds.scan_images_per_sec_min,
  `${metrics.scan_images_per_sec} < ${thresholds.scan_images_per_sec_min}`,
);
check(
  'plan_ms_max',
  metrics.plan_ms <= thresholds.plan_ms_max,
  `${metrics.plan_ms} > ${thresholds.plan_ms_max}`,
);
check(
  'commit_ms_max',
  metrics.commit_ms <= thresholds.commit_ms_max,
  `${metrics.commit_ms} > ${thresholds.commit_ms_max}`,
);
check(
  'commit_images_per_sec_min',
  metrics.commit_images_per_sec >= thresholds.commit_images_per_sec_min,
  `${metrics.commit_images_per_sec} < ${thresholds.commit_images_per_sec_min}`,
);
check(
  'recovery_scan_empty_ms_max',
  metrics.recovery_scan_empty_ms <= thresholds.recovery_scan_empty_ms_max,
  `${metrics.recovery_scan_empty_ms} > ${thresholds.recovery_scan_empty_ms_max}`,
);
check(
  'total_ms_max',
  metrics.total_ms <= thresholds.total_ms_max,
  `${metrics.total_ms} > ${thresholds.total_ms_max}`,
);

const report = {
  gate: 'M9 performance and stability thresholds',
  generated_at: new Date().toISOString(),
  command: 'pnpm release:performance',
  environment: {
    platform: process.platform,
    node: process.version,
    image_count: imageCount,
    postgres_bin: env.IMAGEDB_POSTGRES_BIN ? 'configured' : 'missing',
  },
  thresholds,
  metrics,
  status: failures.length === 0 ? 'passed' : 'failed',
  failures,
  notes: [
    'This MVP gate uses real managed PostgreSQL, real filesystem IO, and command-facing scan/plan/commit/recovery paths.',
    'The automated gate records a 120-image baseline. Larger 1k/10k/100k production benchmarks remain a release-hardening follow-up.',
    'Peak memory is not instrumented by this gate; stability is represented by bounded completion under the thresholded command path.',
  ],
};

mkdirSync(join(repoRoot, 'reports'), { recursive: true });
writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);

if (failures.length > 0) {
  console.error('[m9-performance] failed');
  for (const failure of failures) console.error(`- ${failure}`);
  console.error(`[m9-performance] report: ${reportPath}`);
  process.exit(1);
}

console.log('[m9-performance] passed');
console.log(`[m9-performance] report: ${reportPath}`);
