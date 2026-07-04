import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { App } from './App';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockImplementation((cmd: string) => {
    if (cmd === 'get_app_status') {
      return Promise.resolve('Rust Core 已连接');
    }
    if (cmd === 'get_settings') {
      return Promise.resolve({
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
    }
    if (cmd === 'get_database_status') {
      return Promise.resolve({
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
      });
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

test('renders dashboard page with title', () => {
  renderApp();
  expect(screen.getByRole('heading', { name: '工作台' })).toBeInTheDocument();
});

test('renders sidebar navigation', () => {
  renderApp();
  expect(screen.getByRole('button', { name: '工作台' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '新建导入' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '设置' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '技术探针' })).toBeInTheDocument();
});

test('renders ImageDB brand in sidebar', () => {
  renderApp();
  expect(screen.getByText('ImageDB')).toBeInTheDocument();
});

test('renders status cards section', () => {
  renderApp();
  expect(screen.getByText('数据库')).toBeInTheDocument();
  expect(screen.getByText('pgvector')).toBeInTheDocument();
  expect(screen.getByText('迁移')).toBeInTheDocument();
});

test('renders tagged database status objects without crashing', async () => {
  renderApp();
  expect(await screen.findByText(/缺少 PostgreSQL 运行文件/)).toBeInTheDocument();
});
