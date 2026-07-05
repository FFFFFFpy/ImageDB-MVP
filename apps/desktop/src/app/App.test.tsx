import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { App } from './App';

const mockState = vi.hoisted(() => ({
  settings: {
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
  } as Record<string, unknown>,
  databaseStatus: {
    mode: 'managed_local',
    status: { BinariesMissing: 'pg_ctl/initdb/psql not found' },
    managed_config: {
      data_dir: '/tmp/pgdata',
      port: 5432,
      username: 'imagedb',
      database: 'imagedb',
    },
    external_config: null,
    pgvector_available: true,
    migration_version: '0002_indexes',
    diagnostics: [],
  } as Record<string, unknown>,
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi
    .fn()
    .mockImplementation((cmd: string, args?: { settings?: typeof mockState.settings }) => {
      if (cmd === 'get_app_status') {
        return Promise.resolve('Rust Core 已连接');
      }
      if (cmd === 'get_settings') {
        return Promise.resolve(mockState.settings);
      }
      if (cmd === 'update_settings') {
        mockState.settings = { ...mockState.settings, ...args?.settings };
        return Promise.resolve(mockState.settings);
      }
      if (cmd === 'get_database_status') {
        return Promise.resolve(mockState.databaseStatus);
      }
      if (cmd === 'probe_postgres') {
        return Promise.resolve({
          available: false,
          managed: false,
          pgvector_available: false,
          port: null,
          data_dir: null,
          database_created: false,
          connection_ok: false,
          diagnostics: ['PostgreSQL binaries not found'],
        });
      }
      if (cmd === 'probe_image_fingerprint') {
        return Promise.resolve({
          fingerprints: [],
          diagnostics: ['Test mode'],
          success: false,
        });
      }
      if (cmd === 'probe_file_transaction') {
        return Promise.resolve({
          transaction_id: 'test-id',
          state: 'PUBLISHED',
          source_files: ['test.txt'],
          published_files: ['published/test.txt'],
          blake3_verified: true,
          manifest_path: '/tmp/manifest.json',
          diagnostics: ['Test mode'],
        });
      }
      return Promise.resolve(null);
    }),
}));

beforeEach(() => {
  mockState.settings = {
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
  };
  mockState.databaseStatus = {
    mode: 'managed_local',
    status: { BinariesMissing: 'pg_ctl/initdb/psql not found' },
    managed_config: {
      data_dir: '/tmp/pgdata',
      port: 5432,
      username: 'imagedb',
      database: 'imagedb',
    },
    external_config: null,
    pgvector_available: true,
    migration_version: '0002_indexes',
    diagnostics: [],
  };
});

afterEach(() => cleanup());

function renderApp() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <App />
    </QueryClientProvider>,
  );
}

test('renders dashboard page with title', async () => {
  renderApp();
  expect(await screen.findByRole('heading', { name: '工作台' })).toBeInTheDocument();
});

test('renders sidebar navigation', async () => {
  renderApp();
  expect(await screen.findByRole('button', { name: '工作台' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '新建导入' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '设置' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '技术探针' })).toBeInTheDocument();
});

test('renders ImageDB brand in sidebar', async () => {
  renderApp();
  expect(await screen.findByText('ImageDB')).toBeInTheDocument();
});

test('renders status cards section', async () => {
  renderApp();
  expect(await screen.findByText('数据库')).toBeInTheDocument();
  expect(screen.getByText('pgvector')).toBeInTheDocument();
  expect(screen.getByText('迁移')).toBeInTheDocument();
});

test('renders tagged database status objects without crashing', async () => {
  renderApp();
  expect(await screen.findByText(/缺少 PostgreSQL 运行文件/)).toBeInTheDocument();
});

test('shows managed PostgreSQL startup retries as recovering', async () => {
  mockState.databaseStatus = {
    ...mockState.databaseStatus,
    status: { Error: 'Managed PostgreSQL failed to start' },
    pgvector_available: false,
    migration_version: null,
  };

  renderApp();

  expect(await screen.findByText('托管 PostgreSQL 正在启动 / 恢复中')).toBeInTheDocument();
});

test('shows enter app button when onboarding sees uppercase Connected status', async () => {
  mockState.settings = {
    ...mockState.settings,
    first_run_completed: false,
  };
  mockState.databaseStatus = {
    ...mockState.databaseStatus,
    status: 'Connected',
    pgvector_available: true,
    migration_version: '0010_library_root_leases',
  };

  renderApp();

  expect(await screen.findByRole('button', { name: '进入应用' })).toBeInTheDocument();
});
