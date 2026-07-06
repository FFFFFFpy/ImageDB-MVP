import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { OnboardingPage } from './OnboardingPage';
import { api } from '../lib/ipc/api';

vi.mock('../lib/ipc/api', () => ({
  api: {
    getDatabaseStatus: vi.fn(),
    initializeManagedDatabase: vi.fn(),
    initializeExternalDatabase: vi.fn(),
    testExternalConnection: vi.fn(),
  },
}));

const mockedApi = vi.mocked(api);

beforeEach(() => {
  mockedApi.getDatabaseStatus.mockResolvedValue({
    mode: null,
    status: 'not_initialized',
    managed_config: {
      data_dir: 'C:/Users/Helw/AppData/Local/ImageDB/postgres_data',
      port: 0,
      username: 'imagedb',
      database: 'imagedb',
    },
    external_config: null,
    pgvector_available: false,
    migration_version: null,
    diagnostics: [],
  });
  mockedApi.initializeExternalDatabase.mockResolvedValue({
    mode: 'external',
    status: { Error: 'External database preflight failed; active profile was not switched' },
    managed_config: null,
    external_config: {
      host: '192.168.31.25',
      port: 35973,
      database: 'image_db',
      username: 'helw',
      tls_mode: 'disable',
      ca_cert_path: null,
      client_cert_path: null,
      client_key_path: null,
      connect_timeout_secs: 10,
      query_timeout_secs: 15,
      profile_name: 'default',
    },
    pgvector_available: false,
    migration_version: null,
    diagnostics: ['pgvector extension is not available'],
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function renderOnboardingPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <OnboardingPage onComplete={vi.fn()} />
    </QueryClientProvider>,
  );
}

describe('OnboardingPage database mode flow', () => {
  test('keeps failed external initialization in the external setup context', async () => {
    renderOnboardingPage();

    fireEvent.click(await screen.findByRole('heading', { name: '外部连接' }));
    expect(screen.getByText(/正在配置外部 PostgreSQL/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '连接并初始化' }));

    expect(await screen.findByText(/外部库尚未就绪/)).toBeInTheDocument();
    expect(screen.getAllByText('外部').length).toBeGreaterThan(0);
    expect(screen.getAllByText(/pgvector extension is not available/).length).toBeGreaterThan(0);
    expect(screen.queryByText('请选择数据库模式以完成初始设置。')).not.toBeInTheDocument();
  });
});
