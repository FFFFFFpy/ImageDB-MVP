import { invoke } from '@tauri-apps/api/core';
import type {
  AppSettings,
  DatabaseState,
  ExternalCheckResult,
  ExternalConnectionConfig,
  AllProbeResults,
  PostgresProbeResult,
  ImageFingerprintProbeResult,
  FileTransactionProbeResult,
  ScanProgress,
  ScanSourceInfo,
} from './types';

export const api = {
  getAppStatus: () => invoke<string>('get_app_status'),

  getDatabaseStatus: () => invoke<DatabaseState>('get_database_status'),

  initializeManagedDatabase: () => invoke<DatabaseState>('initialize_managed_database'),

  testExternalConnection: (config: ExternalConnectionConfig) =>
    invoke<ExternalCheckResult>('test_external_connection', { config }),

  initializeExternalDatabase: (config: ExternalConnectionConfig) =>
    invoke<DatabaseState>('initialize_external_database', { config }),

  shutdownDatabase: () => invoke<void>('shutdown_database'),

  getSettings: () => invoke<AppSettings>('get_settings'),

  updateSettings: (settings: AppSettings) => invoke<AppSettings>('update_settings', { settings }),

  probePostgres: () => invoke<PostgresProbeResult>('probe_postgres'),

  probeFingerprint: () => invoke<ImageFingerprintProbeResult>('probe_image_fingerprint'),

  probeFileTransaction: () => invoke<FileTransactionProbeResult>('probe_file_transaction'),

  runAllProbes: () => invoke<AllProbeResults>('run_all_probes'),

  validateSourceDirectory: (sourceRoot: string) =>
    invoke<ScanSourceInfo>('validate_source_directory', { sourceRoot }),

  startScan: (sourceRoot: string) => invoke<string>('start_scan', { sourceRoot }),

  cancelScan: () => invoke<string>('cancel_scan'),

  getScanProgress: () => invoke<ScanProgress>('get_scan_progress'),
};
