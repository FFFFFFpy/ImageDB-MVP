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
    migration_version: '0009_drop_redundant_snapshot_hash',
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
      migration_version: '0009_drop_redundant_snapshot_hash',
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
});
