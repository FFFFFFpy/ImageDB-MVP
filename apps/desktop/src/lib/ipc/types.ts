export type TaggedStatus = string | Record<string, string>;
export type DiagnosticItem = string | number | boolean | null | Record<string, unknown>;

export interface BuildInfo {
  app_version: string;
  git_commit: string;
  git_dirty: boolean | null;
}

export interface DatabaseState {
  mode: 'managed_local' | 'external' | null;
  status: TaggedStatus;
  managed_config: ManagedDbConfig | null;
  external_config: ExternalConnectionConfig | null;
  pgvector_available: boolean;
  migration_version: string | null;
  diagnostics: DiagnosticItem[];
}

export interface CriticalOperationGuardStatus {
  is_blocked: boolean;
  blocking_reason: string | null;
  active_task_kinds: string[];
  active_operation: string | null;
}

export interface DatabaseResetSummary {
  previous_import_runs: number;
  previous_library_albums: number;
  previous_library_images: number;
  previous_file_transactions: number;
  migrations_applied: number;
  migration_version: string;
  filesystem_untouched: boolean;
}

export interface DiagnosticsExportResult {
  path: string;
  generated_at: string;
  file_count: number;
  redacted: boolean;
  byte_size: number;
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

export interface ExternalMigrationProgress extends ExternalMigrationResult {
  state: string;
  current_stage: string;
  errors: string[];
  cancel_requested: boolean;
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

export type CapabilityStatus = 'supported' | 'unsupported' | 'unknown';
export type PublishStrategy = 'strong_local' | 'conservative_mounted' | 'unsupported';
export type StorageType = 'mounted_shared' | 'unknown';

export interface CapabilityProbe {
  status: CapabilityStatus;
  detail: string;
}

export interface StorageCapabilities {
  root: string;
  probe_version: number;
  probed_at: string;
  storage_type: StorageType;
  publish_strategy: PublishStrategy;
  strategy_reasons: string[];
  probe_dir_cleaned: boolean;
  readable: CapabilityProbe;
  writable: CapabilityProbe;
  can_create_dir: CapabilityProbe;
  same_dir_file_rename: CapabilityProbe;
  same_root_rename: CapabilityProbe;
  directory_rename: CapabilityProbe;
  overwrite_rename: CapabilityProbe;
  file_sync_all: CapabilityProbe;
  parent_dir_sync: CapabilityProbe;
  case_sensitive: CapabilityProbe;
  unicode_normalization: CapabilityProbe;
  max_path: CapabilityProbe;
  max_component: CapabilityProbe;
  file_lock: CapabilityProbe;
  timestamp_precision: CapabilityProbe;
  free_space: CapabilityProbe;
  volume_identity: CapabilityProbe;
  diagnostics: string[];
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
  blake3_bytes: number;
  pixel_hash_bytes: number;
  block_hash_bits: number;
  double_gradient_hash_bits: number;
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

export type AlbumWorkflowState =
  'pending' | 'analyzing' | 'analyzed' | 'review_required' | 'failed';

export interface ImportAlbumStatus {
  id: string;
  import_run_id: string;
  source_name: string;
  source_path: string;
  state: AlbumWorkflowState;
  image_count: number;
  fingerprinted_count: number;
  duplicate_candidate_count: number;
  review_candidate_count: number;
  last_error_message: string | null;
  analysis_started_at: string | null;
  analysis_completed_at: string | null;
}

export interface ImportRunDashboard {
  import_run_id: string;
  source_root: string;
  state: string;
  total_albums: number;
  pending_albums: number;
  analyzing_albums: number;
  analyzed_albums: number;
  review_required_albums: number;
  failed_albums: number;
  total_images: number;
  pending_reviews: number;
  duplicate_candidates: number;
}

export type DashboardNextAction =
  | 'recover'
  | 'inspect_transaction_failure'
  | 'review'
  | 'generate_plan'
  | 'resume_analysis'
  | 'inspect_failed'
  | 'resume_commit'
  | 'new_import';

export interface DashboardActionableRun extends ImportRunDashboard {
  next_action: DashboardNextAction;
  has_frozen_plan: boolean;
  has_recoverable_transaction: boolean;
  has_terminal_unresolved_transaction: boolean;
  has_missing_plan_album_transaction: boolean;
}

export interface DatabaseInfoDashboard {
  database: {
    mode: 'managed_local' | 'external' | null;
    status: string;
    pgvector_available: boolean;
    migration_version: string | null;
  };
  library: {
    library_root_count: number;
    library_album_count: number;
    library_image_count: number;
  };
  imports: {
    import_run_count: number;
    import_album_count: number;
    import_image_count: number;
    pending_review_count: number;
    failed_album_count: number;
    recovery_required_run_count: number;
    failed_run_count: number;
    frozen_plan_count: number;
  };
  latest_run: ImportRunDashboard | null;
  latest_actionable_run: DashboardActionableRun | null;
  next_action: DashboardNextAction;
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
  block_distance: number | null;
  double_gradient_distance: number | null;
  block_distance_ratio: number | null;
  double_gradient_distance_ratio: number | null;
  transform_type: string | null;
  confidence: number | null;
  album_name: string;
  album_id: string;
  existing_decision: string | null;
}

export interface ReviewProgress {
  import_run_id: string;
  total_review_groups: number;
  resolved_count: number;
  remaining_count: number;
  all_decided: boolean;
}

export interface ReviewGroupSummary {
  group_id: string;
  state: 'pending' | 'resolved';
  requires_manual_review: boolean;
  member_count: number;
  import_member_count: number;
  library_member_count: number;
  kept_count: number;
}

export type ReviewGroupMemberAction = 'keep' | 'exclude';

export interface ReviewGroupMember {
  image_id: string;
  image_source: 'import' | 'library';
  final_action: ReviewGroupMemberAction;
  decision_source: 'automatic' | 'user';
  source_path: string;
  relative_path: string;
  album_name: string;
  file_size: number;
  width: number | null;
  height: number | null;
  format: string | null;
}

export interface ReviewGroupEvidence {
  candidate_id: string;
  source_image_id: string;
  candidate_image_id: string;
  candidate_image_source: 'import' | 'library';
  scope: string;
  match_type: string;
  blake3_equal: boolean;
  pixel_hash_equal: boolean;
  block_distance: number | null;
  double_gradient_distance: number | null;
  block_distance_ratio: number | null;
  double_gradient_distance_ratio: number | null;
  transform_type: string | null;
  confidence: number | null;
  automatic: boolean;
}

export interface ReviewGroupDetail {
  group_id: string;
  state: 'pending' | 'resolved';
  requires_manual_review: boolean;
  members: ReviewGroupMember[];
  evidence: ReviewGroupEvidence[];
}

export interface ReviewGroupMemberDecision {
  image_id: string;
  image_source: 'import';
  final_action: ReviewGroupMemberAction;
}

export type SourceFileMode = 'copy_and_archive' | 'move_selected_without_backup';

export interface ImportPlanImage {
  image_id: string;
  source_path: string;
  relative_path: string;
  file_size: number;
  source_album_name: string;
  album_name: string;
  album_id: string;
  source_album_id: string;
  included: boolean;
}

export interface ImportPlanAlbum {
  album_id: string;
  source_album_name: string;
  album_name: string;
  included: boolean;
  image_count: number;
  total_size: number;
  images: ImportPlanImage[];
}

export interface ImportPlan {
  import_run_id: string;
  plan_hash: string | null;
  source_file_mode: SourceFileMode;
  library_root_path: string | null;
  total_albums: number;
  total_images: number;
  kept_images: ImportPlanImage[];
  excluded_count: number;
  skipped_albums: string[];
  albums: ImportPlanAlbum[];
}

export type ImportWorkflowStage =
  | 'analysis'
  | 'review'
  | 'generate_plan'
  | 'plan_draft'
  | 'commit_confirm'
  | 'committing'
  | 'recovery'
  | 'completed'
  | 'failed'
  | 'abandoned';

export interface ImportWorkflowResolution {
  import_run_id: string | null;
  stage: ImportWorkflowStage;
  run_state: string | null;
  plan_state: string | null;
  file_transaction_count: number;
}

export interface LibraryAlbumSummary {
  album_id: string;
  library_root_id: string;
  library_root_path: string;
  display_name: string;
  relative_path: string;
  image_count: number;
  total_size: number;
  state: string;
  committed_at: string;
}

export interface LibraryAlbumPage {
  albums: LibraryAlbumSummary[];
  next_cursor: string | null;
  total_albums: number;
  total_images: number;
  total_size: number;
}

export interface LibraryImageSummary {
  image_id: string;
  relative_path: string;
  file_size: number;
  width: number;
  height: number;
  format: string;
  state: string;
}

export interface LibraryImagePage {
  album_id: string;
  images: LibraryImageSummary[];
  next_cursor: string | null;
  total_images: number;
  total_size: number;
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
  source_file_mode: SourceFileMode;
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
  source_file_mode: SourceFileMode;
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
