export type TaggedStatus = string | Record<string, string>;
export type DiagnosticItem = string | number | boolean | null | Record<string, unknown>;

export interface DatabaseState {
  mode: 'managed_local' | 'external' | null;
  status: TaggedStatus;
  managed_config: ManagedDbConfig | null;
  external_config: ExternalConnectionConfig | null;
  pgvector_available: boolean;
  migration_version: string | null;
  diagnostics: DiagnosticItem[];
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
  tls_mode?: 'disable' | 'require' | 'verify_ca' | 'verify_full';
  ca_cert_path?: string | null;
  client_cert_path?: string | null;
  client_key_path?: string | null;
  connect_timeout_secs?: number;
  query_timeout_secs?: number;
  profile_name?: string | null;
}

export interface ExternalCheckResult {
  connection_ok: boolean;
  version: string | null;
  version_ok: boolean;
  tls_mode: 'disable' | 'require' | 'verify_ca' | 'verify_full';
  tls_ok: boolean;
  pgvector_available: boolean;
  can_create_extension: boolean;
  can_create_tables: boolean;
  can_modify_schema: boolean;
  read_write_ok: boolean;
  encoding_ok: boolean;
  timezone_ok: boolean;
  not_read_only: boolean;
  migration_state_ok: boolean;
  schema_compatible: boolean;
  migration_version: string | null;
  checks: ExternalPreflightCheck[];
  diagnostics: DiagnosticItem[];
}

export interface ExternalPreflightCheck {
  code: string;
  status: 'pass' | 'warn' | 'fail';
  message: string;
}

export interface TableRowCount {
  table: string;
  managed_rows: number;
  external_rows: number;
  matches: boolean;
}

export interface ExternalMigrationResult {
  switched: boolean;
  backup_path: string | null;
  migration_version: string | null;
  row_counts: TableRowCount[];
  diagnostics: DiagnosticItem[];
}

export interface AppSettings {
  database_mode: string | null;
  library_root: string | null;
  external_host: string | null;
  external_port: number | null;
  external_database: string | null;
  external_username: string | null;
  external_tls_mode: string | null;
  external_ca_cert_path: string | null;
  external_client_cert_path: string | null;
  external_client_key_path: string | null;
  external_connect_timeout_secs: number | null;
  external_query_timeout_secs: number | null;
  external_profile_name: string | null;
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
  diagnostics: DiagnosticItem[];
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
  diagnostics: DiagnosticItem[];
  success: boolean;
}

export interface FileTransactionProbeResult {
  transaction_id: string;
  state: string;
  source_files: string[];
  published_files: string[];
  blake3_verified: boolean;
  manifest_path: string | null;
  diagnostics: DiagnosticItem[];
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

export interface ReviewCandidateSummary {
  candidate_id: string;
  source_image_id: string;
  candidate_source_image_id: string | null;
  candidate_library_image_id: string | null;
  scope: string;
  match_type: string;
  transform_type: string | null;
  confidence: number | null;
  album_name: string;
  has_decision: boolean;
}

export interface ReviewCandidateDetail {
  candidate_id: string;
  source_image_id: string;
  source_image_path: string;
  source_image_file_size: number;
  source_image_width: number | null;
  source_image_height: number | null;
  candidate_source_image_id: string | null;
  candidate_source_image_path: string | null;
  candidate_source_image_file_size: number | null;
  candidate_source_image_width: number | null;
  candidate_source_image_height: number | null;
  candidate_library_image_id: string | null;
  candidate_library_image_path: string | null;
  candidate_library_image_file_size: number | null;
  candidate_library_image_width: number | null;
  candidate_library_image_height: number | null;
  scope: string;
  match_type: string;
  blake3_equal: boolean;
  pixel_hash_equal: boolean;
  gradient_distance: number | null;
  block_distance: number | null;
  median_distance: number | null;
  transform_type: string | null;
  confidence: number | null;
  album_name: string;
  album_id: string;
  existing_decision: string | null;
}

export interface ReviewProgress {
  import_run_id: string;
  total_review_candidates: number;
  decided_count: number;
  remaining_count: number;
  all_decided: boolean;
}

export interface ImportPlanImage {
  image_id: string;
  source_path: string;
  relative_path: string;
  file_size: number;
  album_name: string;
}

export interface ImportPlan {
  import_run_id: string;
  total_albums: number;
  total_images: number;
  kept_images: ImportPlanImage[];
  excluded_count: number;
  skipped_albums: string[];
}

export interface ImagePreview {
  data_url: string;
}

export type ReviewDecision = 'keep_source' | 'keep_candidate' | 'keep_all' | 'skip_album';

export interface CommitProgress {
  state: string;
  import_run_id: string;
  current_stage: string;
  current_album: string | null;
  albums_total: number;
  albums_completed: number;
  albums_skipped: number;
  albums_failed: number;
  images_committed: number;
  errors: string[];
}

export interface CommitAlbumResult {
  album_name: string;
  status: string;
  images_committed: number;
  target_path: string | null;
  manifest_path: string | null;
  error: string | null;
}

export interface CommitResult {
  import_run_id: string;
  albums_total: number;
  albums_committed: number;
  albums_skipped: number;
  albums_failed: number;
  images_committed: number;
  album_results: CommitAlbumResult[];
  errors: string[];
  state: string;
}

export interface RecoveryDiagnostic {
  transaction_id: string;
  import_run_id: string;
  import_album_id: string;
  current_state: string;
  staging_path: string | null;
  target_path: string | null;
  manifest_path: string | null;
  staging_exists: boolean;
  target_exists: boolean;
  manifest_exists: boolean;
  plan_hash: string | null;
  last_error: string | null;
  diagnostics: string[];
}

export interface RecoveryOutcome {
  transaction_id: string;
  final_state: string;
  recovered: boolean;
  /** true when the transaction is in a genuine terminal state
   * (source_archived / failed / cancelled). failed/cancelled are
   * terminal-but-not-recovered. */
  terminal: boolean;
  message: string;
}

export interface ReverifyResult {
  transaction_id: string;
  verdict: 'already_committed' | 'conflict' | 'resume' | string;
  message: string;
}
