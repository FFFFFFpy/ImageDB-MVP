export interface DatabaseState {
  mode: 'managed_local' | 'external' | null;
  status: 'not_initialized' | 'initializing' | 'ready' | 'connected' | string;
  managed_config: ManagedDbConfig | null;
  external_config: ExternalConnectionConfig | null;
  pgvector_available: boolean;
  migration_version: string | null;
  diagnostics: string[];
}

export interface ManagedDbConfig {
  data_dir: string;
  port: number;
  username: string;
  database: string;
}

export interface ExternalConnectionConfig {
  host: string;
  port: number;
  database: string;
  username: string;
  password?: string;
}

export interface ExternalCheckResult {
  connection_ok: boolean;
  version: string | null;
  version_ok: boolean;
  pgvector_available: boolean;
  can_create_tables: boolean;
  diagnostics: string[];
}

export interface AppSettings {
  database_mode: string | null;
  library_root: string | null;
  external_host: string | null;
  external_port: number | null;
  external_database: string | null;
  external_username: string | null;
  first_run_completed: boolean;
}

export interface PostgresProbeResult {
  available: boolean;
  managed: boolean;
  pgvector_available: boolean;
  port: number | null;
  data_dir: string | null;
  database_created: boolean;
  connection_ok: boolean;
  diagnostics: string[];
}

export interface ImageFingerprintEntry {
  fingerprint_version: number;
  file_path: string;
  format: string;
  width: number;
  height: number;
  file_size: number;
  blake3: string;
  pixel_hash: string;
  gradient_hash: string;
  block_hash: string;
  median_hash: string;
}

export interface ImageFingerprintProbeResult {
  fingerprints: ImageFingerprintEntry[];
  diagnostics: string[];
  success: boolean;
}

export interface FileTransactionProbeResult {
  transaction_id: string;
  state: string;
  source_files: string[];
  published_files: string[];
  blake3_verified: boolean;
  manifest_path: string | null;
  diagnostics: string[];
}

export interface AllProbeResults {
  postgres: PostgresProbeResult;
  fingerprint: ImageFingerprintProbeResult;
  file_transaction: FileTransactionProbeResult;
}

export interface ScanProgress {
  state: string;
  import_run_id: string | null;
  current_stage: string;
  current_album: string | null;
  processed_images: number;
  total_albums: number;
  total_images: number;
  duplicate_count: number;
  error_count: number;
  errors: string[];
}

export interface ScanSourceInfo {
  path: string;
  albums: string[];
  album_count: number;
}
