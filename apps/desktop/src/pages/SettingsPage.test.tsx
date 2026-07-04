import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { SettingsPage } from './SettingsPage';
import { api } from '../lib/ipc/api';

vi.mock('../lib/ipc/api', () => ({
  api: {
    getSettings: vi.fn(),
    getDatabaseStatus: vi.fn(),
    getExternalMigrationProgress: vi.fn(),
    testExternalConnection: vi.fn(),
    startManagedToExternalMigration: vi.fn(),
    cancelExternalMigration: vi.fn(),
    shutdownDatabase: vi.fn(),
    switchToManagedDatabase: vi.fn(),
    updateSettings: vi.fn(),
    probeStorageCapabilities: vi.fn(),
  },
}));

const mockedApi = vi.mocked(api);

afterEach(() => cleanup());

beforeEach(() => {
  vi.clearAllMocks();
  mockedApi.getSettings.mockResolvedValue({
    database_mode: 'managed_local',
    library_root: null,
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
  });
  mockedApi.getDatabaseStatus.mockResolvedValue({
    mode: 'managed_local',
    status: 'connected',
    managed_config: {
      data_dir: 'C:/imagedb/postgres',
      port: 54329,
      username: 'imagedb',
      database: 'imagedb',
    },
    external_config: null,
    pgvector_available: true,
    migration_version: '0010_library_root_leases',
    diagnostics: ['managed database ready'],
  });
  mockedApi.getExternalMigrationProgress.mockResolvedValue({
    state: 'idle',
    current_stage: 'idle',
    switched: false,
    backup_path: null,
    migration_version: null,
    row_counts: [],
    diagnostics: [],
    errors: [],
    cancel_requested: false,
  });
  mockedApi.probeStorageCapabilities.mockResolvedValue({
    root: 'C:/ImageLibrary',
    probe_version: 1,
    probed_at: '2026-07-04T00:00:00Z',
    storage_type: 'unknown',
    publish_strategy: 'conservative_mounted',
    strategy_reasons: ['parent_dir_sync is Unknown'],
    probe_dir_cleaned: true,
    readable: { status: 'supported', detail: 'root can be read' },
    writable: { status: 'supported', detail: 'file can be created and written' },
    can_create_dir: { status: 'supported', detail: 'dedicated probe directory created' },
    same_dir_file_rename: {
      status: 'supported',
      detail: 'file rename within one directory succeeded',
    },
    same_root_rename: {
      status: 'supported',
      detail: 'file rename across sibling directories succeeded',
    },
    directory_rename: { status: 'supported', detail: 'directory rename succeeded' },
    overwrite_rename: {
      status: 'unsupported',
      detail: 'rename over existing target failed',
    },
    file_sync_all: { status: 'supported', detail: 'file sync_all succeeded' },
    parent_dir_sync: {
      status: 'unknown',
      detail: 'directory sync_all could not be verified',
    },
    case_sensitive: {
      status: 'unsupported',
      detail: 'case variants resolve to the same path',
    },
    unicode_normalization: {
      status: 'supported',
      detail: 'composed and decomposed Unicode names remain distinct',
    },
    max_path: { status: 'supported', detail: 'created path with 280 characters' },
    max_component: {
      status: 'supported',
      detail: 'created a 240-character path component',
    },
    file_lock: { status: 'supported', detail: 'exclusive advisory file lock succeeded' },
    timestamp_precision: {
      status: 'supported',
      detail: 'modified timestamp changed after a 25 ms rewrite',
    },
    free_space: { status: 'supported', detail: '1024 bytes available' },
    volume_identity: { status: 'supported', detail: 'volume_serial_number=1' },
    diagnostics: [],
  });
});

function renderSettingsPage() {
  const client = new QueryClient({
    defaultOptions: {
      mutations: { retry: false },
      queries: { retry: false },
    },
  });

  return render(
    <QueryClientProvider client={client}>
      <SettingsPage />
    </QueryClientProvider>,
  );
}

describe('SettingsPage external PostgreSQL GUI', () => {
  test('renders structured external preflight diagnostics after testing a connection', async () => {
    mockedApi.testExternalConnection.mockResolvedValue({
      connection_ok: true,
      version: 'PostgreSQL 18.4',
      version_ok: true,
      tls_mode: 'verify_full',
      tls_ok: true,
      pgvector_available: true,
      can_create_extension: true,
      can_create_tables: true,
      can_modify_schema: true,
      read_write_ok: true,
      encoding_ok: true,
      timezone_ok: true,
      not_read_only: true,
      migration_state_ok: true,
      schema_compatible: true,
      migration_version: null,
      diagnostics: ['external preflight completed'],
      checks: [
        {
          code: 'postgres.version',
          status: 'pass',
          message: 'PostgreSQL 18.4 is supported',
        },
        {
          code: 'schema.compatibility',
          status: 'pass',
          message: 'ImageDB schema is compatible',
        },
      ],
    });

    renderSettingsPage();

    fireEvent.click(await screen.findByRole('button', { name: '测试连接' }));

    expect(await screen.findByText('PostgreSQL 18.4')).toBeInTheDocument();
    expect(screen.getByText('postgres.version')).toBeInTheDocument();
    expect(screen.getByText('schema.compatibility')).toBeInTheDocument();
    expect(screen.getByText('ImageDB schema is compatible')).toBeInTheDocument();
  });

  test('renders migration progress, backup, row counts, diagnostics, errors, and cancel state', async () => {
    mockedApi.getExternalMigrationProgress.mockResolvedValue({
      state: 'running',
      current_stage: 'verify',
      switched: false,
      backup_path: 'C:/imagedb/postgres_backups/external_migrations/managed-to-external.sql',
      migration_version: '0010_library_root_leases',
      row_counts: [
        {
          table: 'app_meta',
          managed_rows: 3,
          external_rows: 3,
          matches: true,
        },
      ],
      diagnostics: [
        'External migration table content fingerprints verified',
        'External migration constraints and indexes verified',
      ],
      errors: ['external migration cancelled by user; profile not switched'],
      cancel_requested: true,
    });

    renderSettingsPage();

    expect(await screen.findByText('verify')).toBeInTheDocument();
    expect(screen.getByText('未切换')).toBeInTheDocument();
    expect(screen.getByText(/managed-to-external\.sql/)).toBeInTheDocument();
    expect(screen.getByText('app_meta')).toBeInTheDocument();
    expect(screen.getByText('一致')).toBeInTheDocument();
    expect(screen.getByText('迁移诊断 (2)')).toBeInTheDocument();
    expect(
      screen.getByText('External migration constraints and indexes verified'),
    ).toBeInTheDocument();
    expect(
      screen.getByText('external migration cancelled by user; profile not switched'),
    ).toBeInTheDocument();

    await waitFor(() => {
      expect(screen.getByRole('button', { name: '取消迁移' })).toBeEnabled();
    });
  });

  test('renders mounted storage capability report after probing the library root', async () => {
    renderSettingsPage();

    const input = await screen.findByLabelText('目标图库根目录');
    fireEvent.change(input, { target: { value: 'C:/ImageLibrary' } });
    fireEvent.click(screen.getByRole('button', { name: '检测存储能力' }));

    expect(await screen.findByText('保守可恢复')).toBeInTheDocument();
    expect(screen.getByText('文件同步')).toBeInTheDocument();
    expect(screen.getByText(/file sync_all succeeded/)).toBeInTheDocument();
    expect(screen.getByText('父目录同步')).toBeInTheDocument();
    expect(screen.getByText(/directory sync_all could not be verified/)).toBeInTheDocument();
    expect(screen.getByText('策略依据 (1)')).toBeInTheDocument();
    expect(mockedApi.probeStorageCapabilities).toHaveBeenCalledWith('C:/ImageLibrary');
  });
});
