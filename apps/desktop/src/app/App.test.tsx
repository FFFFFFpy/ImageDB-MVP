import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { App } from './App';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockImplementation((cmd: string) => {
    if (cmd === 'get_app_status') {
      return Promise.resolve('Rust Core 已连接');
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

test('renders probe page title', () => {
  renderApp();
  expect(screen.getByText('技术探针 - Milestone 0')).toBeInTheDocument();
});

test('renders connection test button', () => {
  renderApp();
  expect(screen.getByRole('button', { name: '连接测试' })).toBeInTheDocument();
});

test('renders all probe tabs', () => {
  renderApp();
  expect(screen.getByRole('button', { name: '数据库' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '图片指纹' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '文件事务' })).toBeInTheDocument();
});

test('renders run all probes button', () => {
  renderApp();
  expect(screen.getByRole('button', { name: '运行全部探针' })).toBeInTheDocument();
});
