import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type {
  AppSettings,
  CriticalOperationGuardStatus,
  DatabaseState,
  ExternalMigrationProgress,
} from '../../lib/ipc/types';
import { SettingsPage } from '../../pages/SettingsPage';
import { Layout } from '../Layout';

export const settingsDatabaseState: DatabaseState = {
  mode: 'managed_local',
  status: 'connected',
  managed_config: {
    data_dir: 'C:/Users/Helw/AppData/Local/ImageDB/postgres_data',
    port: 54321,
    username: 'imagedb',
    database: 'imagedb',
  },
  external_config: null,
  pgvector_available: true,
  migration_version: '202607130001',
  diagnostics: [],
};

const settings: AppSettings = {
  database_mode: 'managed_local',
  library_root: 'D:/ImageLibrary（个人图库）',
  external_host: null,
  external_port: null,
  external_database: null,
  external_username: null,
  external_tls_mode: null,
  external_ca_cert_path: null,
  external_client_cert_path: null,
  external_client_key_path: null,
  external_connect_timeout_secs: null,
  external_query_timeout_secs: null,
  external_profile_name: null,
  first_run_completed: true,
};

const migration: ExternalMigrationProgress = {
  state: 'idle',
  current_stage: 'idle',
  switched: false,
  backup_path: null,
  migration_version: null,
  row_counts: [],
  diagnostics: [],
  errors: [],
  cancel_requested: false,
};

const criticalOperationGuard: CriticalOperationGuardStatus = {
  is_blocked: false,
  blocking_reason: null,
  active_task_kinds: [],
  active_operation: null,
};

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['settings'], settings);
fixtureClient.setQueryData(['database-status'], settingsDatabaseState);
fixtureClient.setQueryData(['external-migration-progress'], migration);
fixtureClient.setQueryData(['critical-operation-guard-status'], criticalOperationGuard);
fixtureClient.setQueryData(['database-info-dashboard'], {
  imports: { failed_album_count: 0, pending_review_count: 0, recovery_required_run_count: 0 },
});

export function SettingsFixture() {
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout currentRoute="settings" onNavigate={() => undefined} enablePolling={false}>
        <SettingsPage enablePolling={false} onOpenProbes={() => undefined} />
      </Layout>
    </QueryClientProvider>
  );
}
