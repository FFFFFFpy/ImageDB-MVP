import { invoke } from '@tauri-apps/api/core';
import type {
  AppSettings,
  BuildInfo,
  DatabaseState,
  DatabaseResetSummary,
  CriticalOperationGuardStatus,
  DatabaseInfoDashboard,
  DiagnosticsExportResult,
  ExternalCheckResult,
  ExternalConnectionConfig,
  AllProbeResults,
  PostgresProbeResult,
  ImageFingerprintProbeResult,
  FileTransactionProbeResult,
  ImportAlbumStatus,
  ImportRunDashboard,
  ScanProgress,
  ScanSourceInfo,
  ReviewCandidateSummary,
  ReviewCandidateDetail,
  ReviewProgress,
  ReviewGroupSummary,
  ReviewGroupDetail,
  ReviewGroupMemberDecision,
  SourceFileMode,
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
  LibraryAlbumPage,
  LibraryImagePage,
} from './types';

export const api = {
  getAppStatus: () => invoke<string>('get_app_status'),

  getBuildInfo: () => invoke<BuildInfo>('get_build_info'),

  getDatabaseStatus: () => invoke<DatabaseState>('get_database_status'),

  getCriticalOperationGuardStatus: () =>
    invoke<CriticalOperationGuardStatus>('get_critical_operation_guard_status'),

  getDatabaseInfoDashboard: () => invoke<DatabaseInfoDashboard>('get_database_info_dashboard'),

  getLibraryAlbums: (cursor: string | null, limit: number) =>
    invoke<LibraryAlbumPage>('get_library_albums', { cursor, limit }),

  getLibraryImages: (albumId: string, cursor: string | null, limit: number) =>
    invoke<LibraryImagePage>('get_library_images', { albumId, cursor, limit }),

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

  resetDatabaseHistory: (confirmation: string) =>
    invoke<DatabaseResetSummary>('reset_database_history', { confirmation }),

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
  selectSourceDirectory: () => invoke<string | null>('select_source_directory'),

  startScan: (sourceRoot: string) => invoke<string>('start_scan', { sourceRoot }),

  cancelScan: () => invoke<string>('cancel_scan'),

  getScanProgress: () => invoke<ScanProgress>('get_scan_progress'),

  getImportRunsDashboard: () => invoke<ImportRunDashboard[]>('get_import_runs_dashboard'),

  getImportRunAlbums: (importRunId: string) =>
    invoke<ImportAlbumStatus[]>('get_import_run_albums', { importRunId }),

  resumeImportRun: (importRunId: string) => invoke<string>('resume_import_run', { importRunId }),

  retryImportAlbum: (albumId: string) =>
    invoke<ImportAlbumStatus>('retry_import_album', { albumId }),

  abandonImportRun: (importRunId: string) => invoke<void>('abandon_import_run', { importRunId }),

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

  getReviewGroups: (importRunId: string) =>
    invoke<ReviewGroupSummary[]>('get_review_groups', { importRunId }),

  getReviewGroupDetail: (groupId: string) =>
    invoke<ReviewGroupDetail>('get_review_group_detail', { groupId }),

  submitReviewGroupDecision: (groupId: string, decisions: ReviewGroupMemberDecision[]) =>
    invoke<void>('submit_review_group_decision', { groupId, decisions }),

  generateImportPlan: (importRunId: string) =>
    invoke<ImportPlan>('generate_import_plan', { importRunId }),

  freezeImportPlan: (importRunId: string) =>
    invoke<ImportPlan>('freeze_import_plan', { importRunId }),

  getFrozenImportPlanSummary: (importRunId: string) =>
    invoke<ImportPlan | null>('get_frozen_import_plan_summary', { importRunId }),

  abandonFrozenImportWorkflow: (importRunId: string) =>
    invoke<void>('abandon_frozen_import_workflow', { importRunId }),

  setImportPlanAlbumIncluded: (importRunId: string, albumId: string, included: boolean) =>
    invoke<ImportPlan>('set_import_plan_album_included', { importRunId, albumId, included }),

  setImportPlanImageIncluded: (
    importRunId: string,
    imageId: string,
    targetAlbumId: string,
    included: boolean,
  ) =>
    invoke<ImportPlan>('set_import_plan_image_included', {
      importRunId,
      imageId,
      targetAlbumId,
      included,
    }),

  setImportPlanSourceFileMode: (importRunId: string, sourceFileMode: SourceFileMode) =>
    invoke<ImportPlan>('set_import_plan_source_file_mode', { importRunId, sourceFileMode }),

  getLatestCompletedImportRun: () => invoke<string | null>('get_latest_completed_import_run'),

  getLatestReviewableImportRun: () => invoke<string | null>('get_latest_reviewable_import_run'),

  getLatestCommittableImportRun: () => invoke<string | null>('get_latest_committable_import_run'),

  getImagePreview: (candidateId: string, imageSide: string) =>
    invoke<ImagePreview>('get_image_preview', { candidateId, imageSide }),

  getImportPlanImagePreview: (importRunId: string, imageId: string) =>
    invoke<ImagePreview>('get_import_plan_image_preview', { importRunId, imageId }),

  getReviewGroupMemberPreview: (groupId: string, imageId: string, imageSource: string) =>
    invoke<ImagePreview>('get_review_group_member_preview', { groupId, imageId, imageSource }),

  startImportCommit: (importRunId: string, expectedPlanHash: string) =>
    invoke<string>('start_import_commit', { importRunId, expectedPlanHash }),

  cancelImportCommit: () => invoke<string>('cancel_import_commit'),

  getCommitProgress: () => invoke<CommitProgress>('get_commit_progress'),

  scanRecoverableTransactions: () => invoke<RecoveryDiagnostic[]>('scan_recoverable_transactions'),

  recoverTransaction: (transactionId: string) =>
    invoke<RecoveryOutcome>('recover_transaction', { transactionId }),

  reverifyTransaction: (transactionId: string) =>
    invoke<ReverifyResult>('reverify_transaction', { transactionId }),
};
