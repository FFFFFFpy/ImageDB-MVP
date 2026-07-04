import { invoke } from '@tauri-apps/api/core';
import type {
  AppSettings,
  DatabaseState,
  DiagnosticsExportResult,
  ExternalCheckResult,
  ExternalConnectionConfig,
  AllProbeResults,
  PostgresProbeResult,
  ImageFingerprintProbeResult,
  FileTransactionProbeResult,
  ScanProgress,
  ScanSourceInfo,
  ReviewCandidateSummary,
  ReviewCandidateDetail,
  ReviewProgress,
  ImportPlan,
  ImagePreview,
  ReviewDecision,
  CommitProgress,
  RecoveryDiagnostic,
  RecoveryOutcome,
  ReverifyResult,
  ExternalMigrationResult,
  ExternalMigrationProgress,
  StorageCapabilities,
} from './types';

export const api = {
  getAppStatus: () => invoke<string>('get_app_status'),

  getDatabaseStatus: () => invoke<DatabaseState>('get_database_status'),

  initializeManagedDatabase: () => invoke<DatabaseState>('initialize_managed_database'),

  switchToManagedDatabase: () => invoke<DatabaseState>('switch_to_managed_database'),

  testExternalConnection: (config: ExternalConnectionConfig) =>
    invoke<ExternalCheckResult>('test_external_connection', { config }),

  initializeExternalDatabase: (config: ExternalConnectionConfig) =>
    invoke<DatabaseState>('initialize_external_database', { config }),

  migrateManagedToExternalDatabase: (config: ExternalConnectionConfig) =>
    invoke<ExternalMigrationResult>('migrate_managed_to_external_database', { config }),

  startManagedToExternalMigration: (config: ExternalConnectionConfig) =>
    invoke<string>('start_managed_to_external_migration', { config }),

  cancelExternalMigration: () => invoke<string>('cancel_external_migration'),

  getExternalMigrationProgress: () =>
    invoke<ExternalMigrationProgress>('get_external_migration_progress'),

  shutdownDatabase: () => invoke<void>('shutdown_database'),

  exportDiagnostics: () => invoke<DiagnosticsExportResult>('export_diagnostics'),

  getSettings: () => invoke<AppSettings>('get_settings'),

  updateSettings: (settings: AppSettings) => invoke<AppSettings>('update_settings', { settings }),

  probeStorageCapabilities: (path: string) =>
    invoke<StorageCapabilities>('probe_storage_capabilities', { path }),

  probePostgres: () => invoke<PostgresProbeResult>('probe_postgres'),

  probeFingerprint: () => invoke<ImageFingerprintProbeResult>('probe_image_fingerprint'),

  probeFileTransaction: () => invoke<FileTransactionProbeResult>('probe_file_transaction'),

  runAllProbes: () => invoke<AllProbeResults>('run_all_probes'),

  validateSourceDirectory: (sourceRoot: string) =>
    invoke<ScanSourceInfo>('validate_source_directory', { sourceRoot }),

  startScan: (sourceRoot: string) => invoke<string>('start_scan', { sourceRoot }),

  cancelScan: () => invoke<string>('cancel_scan'),

  getScanProgress: () => invoke<ScanProgress>('get_scan_progress'),

  getReviewQueue: (importRunId: string) =>
    invoke<ReviewCandidateSummary[]>('get_review_queue', { importRunId }),

  getReviewCandidateDetail: (candidateId: string) =>
    invoke<ReviewCandidateDetail>('get_review_candidate_detail', { candidateId }),

  submitReviewDecision: (candidateId: string, decision: ReviewDecision) =>
    invoke<void>('submit_review_decision', { candidateId, decision }),

  skipReviewAlbum: (importRunId: string, albumId: string) =>
    invoke<number>('skip_review_album', { importRunId, albumId }),

  getReviewProgress: (importRunId: string) =>
    invoke<ReviewProgress>('get_review_progress', { importRunId }),

  generateImportPlan: (importRunId: string) =>
    invoke<ImportPlan>('generate_import_plan', { importRunId }),

  freezeImportPlan: (importRunId: string) =>
    invoke<ImportPlan>('freeze_import_plan', { importRunId }),

  getFrozenImportPlanSummary: (importRunId: string) =>
    invoke<ImportPlan | null>('get_frozen_import_plan_summary', { importRunId }),

  getLatestCompletedImportRun: () => invoke<string | null>('get_latest_completed_import_run'),

  getLatestCommittableImportRun: () => invoke<string | null>('get_latest_committable_import_run'),

  getImagePreview: (candidateId: string, imageSide: string) =>
    invoke<ImagePreview>('get_image_preview', { candidateId, imageSide }),

  startImportCommit: (importRunId: string) =>
    invoke<string>('start_import_commit', { importRunId }),

  cancelImportCommit: () => invoke<string>('cancel_import_commit'),

  getCommitProgress: () => invoke<CommitProgress>('get_commit_progress'),

  scanRecoverableTransactions: () => invoke<RecoveryDiagnostic[]>('scan_recoverable_transactions'),

  recoverTransaction: (transactionId: string) =>
    invoke<RecoveryOutcome>('recover_transaction', { transactionId }),

  reverifyTransaction: (transactionId: string) =>
    invoke<ReverifyResult>('reverify_transaction', { transactionId }),
};
