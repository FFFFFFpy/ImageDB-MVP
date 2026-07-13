import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
  databaseInfo: null as Record<string, unknown> | null,
  latestReviewableRunId: 'run-newer-b',
  latestCommittableRunId: 'run-newer-b',
  requestedReviewRunIds: [] as string[],
  requestedPlanRunIds: [] as string[],
  commitRequests: [] as Array<Record<string, unknown>>,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (cmd === 'get_app_status') {
      return Promise.resolve('Rust Core 已连接');
    }
    if (cmd === 'get_settings') {
      return Promise.resolve(mockState.settings);
    }
    if (cmd === 'update_settings') {
      mockState.settings = {
        ...mockState.settings,
        ...(args?.settings as Record<string, unknown> | undefined),
      };
      return Promise.resolve(mockState.settings);
    }
    if (cmd === 'get_database_status') {
      return Promise.resolve(mockState.databaseStatus);
    }
    if (cmd === 'get_database_info_dashboard') {
      return Promise.resolve(mockState.databaseInfo);
    }
    if (cmd === 'get_import_runs_dashboard') {
      return Promise.resolve(mockState.databaseInfo ? [mockState.databaseInfo.latest_run] : []);
    }
    if (cmd === 'get_import_run_albums') {
      return Promise.resolve([]);
    }
    if (cmd === 'get_latest_reviewable_import_run') {
      return Promise.resolve(mockState.latestReviewableRunId);
    }
    if (cmd === 'get_latest_committable_import_run') {
      return Promise.resolve(mockState.latestCommittableRunId);
    }
    if (cmd === 'get_review_queue') {
      return Promise.resolve([]);
    }
    if (cmd === 'get_review_progress') {
      mockState.requestedReviewRunIds.push(String(args?.importRunId));
      return Promise.resolve({
        import_run_id: args?.importRunId,
        total_review_candidates: 0,
        decided_count: 0,
        remaining_count: 0,
        all_decided: false,
      });
    }
    if (cmd === 'get_frozen_import_plan_summary') {
      const importRunId = String(args?.importRunId);
      mockState.requestedPlanRunIds.push(importRunId);
      return Promise.resolve({
        import_run_id: importRunId,
        plan_hash: `hash-${importRunId}`,
        total_albums: 1,
        total_images: 1,
        kept_images: [
          {
            image_id: 'image-a',
            source_path: 'D:/Source/image.jpg',
            relative_path: 'Album/image.jpg',
            file_size: 100,
            album_name: 'Album',
            album_id: 'album-a',
            source_album_id: 'album-a',
            included: true,
          },
        ],
        excluded_count: 0,
        skipped_albums: [],
        albums: [],
      });
    }
    if (cmd === 'start_import_commit') {
      mockState.commitRequests.push(args ?? {});
      return Promise.resolve('commit started');
    }
    if (cmd === 'get_scan_progress') {
      return Promise.resolve({
        state: 'idle',
        import_run_id: null,
        current_stage: 'idle',
        current_album: null,
        processed_images: 0,
        total_albums: 0,
        total_images: 0,
        duplicate_count: 0,
        error_count: 0,
        errors: [],
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

beforeEach(() => {
  window.location.hash = '';
  window.localStorage.clear();
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
  mockState.databaseInfo = null;
  mockState.latestReviewableRunId = 'run-newer-b';
  mockState.latestCommittableRunId = 'run-newer-b';
  mockState.requestedReviewRunIds = [];
  mockState.requestedPlanRunIds = [];
  mockState.commitRequests = [];
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

function setActionableDashboard(nextAction: 'review' | 'resume_commit', importRunId: string) {
  const state = nextAction === 'review' ? 'review_required' : 'ready_to_commit';
  const run = {
    import_run_id: importRunId,
    source_root: 'D:/Selected',
    state,
    total_albums: 1,
    pending_albums: 0,
    analyzing_albums: 0,
    analyzed_albums: 1,
    review_required_albums: nextAction === 'review' ? 1 : 0,
    failed_albums: 0,
    total_images: 1,
    pending_reviews: nextAction === 'review' ? 1 : 0,
    duplicate_candidates: nextAction === 'review' ? 1 : 0,
  };
  mockState.databaseStatus = { ...mockState.databaseStatus, status: 'Connected' };
  mockState.databaseInfo = {
    database: {
      mode: 'managed_local',
      status: 'Connected',
      pgvector_available: true,
      migration_version: '0012_album_workflow_repair',
    },
    library: { library_root_count: 1, library_album_count: 0, library_image_count: 0 },
    imports: {
      import_run_count: 2,
      import_album_count: 2,
      import_image_count: 2,
      pending_review_count: nextAction === 'review' ? 1 : 0,
      failed_album_count: 0,
      recovery_required_run_count: 0,
      failed_run_count: 0,
      frozen_plan_count: nextAction === 'resume_commit' ? 2 : 1,
    },
    latest_run: { ...run, import_run_id: 'run-newer-b' },
    next_action: nextAction,
    latest_actionable_run: {
      ...run,
      next_action: nextAction,
      has_frozen_plan: nextAction === 'resume_commit',
      has_recoverable_transaction: false,
      has_terminal_unresolved_transaction: false,
      has_missing_plan_album_transaction: false,
    },
  };
}

test('keeps the dashboard-selected review run when a newer second run exists', async () => {
  setActionableDashboard('review', 'run-older-a');
  renderApp();

  fireEvent.click(await screen.findByRole('button', { name: '继续审核' }));

  expect(await screen.findByRole('heading', { name: '审核' })).toBeInTheDocument();
  await waitFor(() => expect(mockState.requestedReviewRunIds).toContain('run-older-a'));
  expect(mockState.requestedReviewRunIds).not.toContain('run-newer-b');
});

test('keeps the dashboard-selected commit run when a newer second run exists', async () => {
  setActionableDashboard('resume_commit', 'run-older-a');
  renderApp();

  fireEvent.click(await screen.findByRole('button', { name: '继续入库' }));

  expect(await screen.findByRole('heading', { name: '提交入库' })).toBeInTheDocument();
  await waitFor(() => expect(mockState.requestedPlanRunIds).toContain('run-older-a'));
  expect(mockState.requestedPlanRunIds).not.toContain('run-newer-b');
  expect(screen.getByText('计划哈希：hash-run-older-a')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '确认并开始入库' }));
  await waitFor(() =>
    expect(mockState.commitRequests).toContainEqual({
      importRunId: 'run-older-a',
      expectedPlanHash: 'hash-run-older-a',
    }),
  );
});

test('renders dashboard page with title', async () => {
  renderApp();
  expect(await screen.findByRole('heading', { name: '工作台' })).toBeInTheDocument();
});

test('renders sidebar navigation', async () => {
  renderApp();
  expect(await screen.findByRole('button', { name: '工作台' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '新建导入' })).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '设置' })).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: '技术探针' })).not.toBeInTheDocument();

  fireEvent.click(screen.getByRole('button', { name: '设置' }));
  expect(await screen.findByRole('button', { name: '打开技术探针' })).toBeInTheDocument();
});

test('sidebar new import clears a run selected from the dashboard', async () => {
  mockState.databaseStatus = {
    ...mockState.databaseStatus,
    status: 'Connected',
  };
  mockState.databaseInfo = {
    database: {
      mode: 'managed_local',
      status: 'Connected',
      pgvector_available: true,
      migration_version: '0012_album_workflow_repair',
    },
    library: {
      library_root_count: 1,
      library_album_count: 0,
      library_image_count: 0,
    },
    imports: {
      import_run_count: 1,
      import_album_count: 1,
      import_image_count: 1,
      pending_review_count: 0,
      failed_album_count: 0,
      recovery_required_run_count: 0,
      failed_run_count: 0,
      frozen_plan_count: 0,
    },
    latest_run: {
      import_run_id: 'run-selected',
      source_root: 'D:/Selected',
      state: 'analyzing',
      total_albums: 1,
      pending_albums: 1,
      analyzing_albums: 0,
      analyzed_albums: 0,
      review_required_albums: 0,
      failed_albums: 0,
      total_images: 1,
      pending_reviews: 0,
      duplicate_candidates: 0,
    },
    next_action: 'resume_analysis',
    latest_actionable_run: {
      import_run_id: 'run-selected',
      source_root: 'D:/Selected',
      state: 'analyzing',
      total_albums: 1,
      pending_albums: 1,
      analyzing_albums: 0,
      analyzed_albums: 0,
      review_required_albums: 0,
      failed_albums: 0,
      total_images: 1,
      pending_reviews: 0,
      duplicate_candidates: 0,
      next_action: 'resume_analysis',
      has_frozen_plan: false,
      has_recoverable_transaction: false,
      has_terminal_unresolved_transaction: false,
      has_missing_plan_album_transaction: false,
    },
  };

  renderApp();

  fireEvent.click(await screen.findByRole('button', { name: '继续分析' }));
  expect(await screen.findByRole('heading', { name: '新建导入' })).toBeInTheDocument();
  expect(await screen.findByDisplayValue('D:/Selected')).toBeInTheDocument();

  fireEvent.click(screen.getByRole('button', { name: '新建导入' }));

  expect(screen.queryByRole('button', { name: '继续分析' })).not.toBeInTheDocument();
  expect(await screen.findByText('暂无图集状态。验证源目录后开始分析。')).toBeInTheDocument();
});

test('renders ImageDB brand in sidebar', async () => {
  renderApp();
  expect(await screen.findByText('ImageDB')).toBeInTheDocument();
});

test('renders compact system health section', async () => {
  renderApp();
  expect(await screen.findByRole('region', { name: '系统健康' })).toBeInTheDocument();
  expect(screen.getByText('pgvector 可用')).toBeInTheDocument();
  expect(screen.getByText(/迁移 0002_indexes/)).toBeInTheDocument();
});

test('does not present a local managed path as active before database mode is selected', async () => {
  mockState.databaseStatus = {
    ...mockState.databaseStatus,
    mode: null,
    status: 'not_initialized',
    managed_config: {
      data_dir: 'C:/Users/Helw/AppData/Local/ImageDB/postgres_data',
      port: 0,
      username: 'imagedb',
      database: 'imagedb',
    },
    pgvector_available: false,
    migration_version: null,
  };

  renderApp();

  expect((await screen.findAllByText('未初始化')).length).toBeGreaterThan(0);
  expect(screen.getByText('尚未选择数据库模式')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '选择数据库模式' })).toBeInTheDocument();
  expect(screen.queryByText(/postgres_data : 0/)).not.toBeInTheDocument();
});

test('routes unresolved database mode from dashboard to settings instead of onboarding loop', async () => {
  mockState.databaseStatus = {
    ...mockState.databaseStatus,
    mode: null,
    status: 'not_initialized',
    managed_config: {
      data_dir: 'C:/Users/Helw/AppData/Local/ImageDB/postgres_data',
      port: 0,
      username: 'imagedb',
      database: 'imagedb',
    },
    pgvector_available: false,
    migration_version: null,
  };

  renderApp();

  fireEvent.click(await screen.findByRole('button', { name: '选择数据库模式' }));

  expect(await screen.findByRole('heading', { name: '设置' })).toBeInTheDocument();
  expect(screen.getByText('外部数据库连接')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '初始化托管库' })).toBeInTheDocument();
});

test('renders tagged database status objects without crashing', async () => {
  renderApp();
  expect((await screen.findAllByText(/缺少 PostgreSQL 运行文件/)).length).toBeGreaterThan(0);
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

test('entering the app after external initialization does not overwrite freshly stored settings', async () => {
  mockState.settings = {
    ...mockState.settings,
    database_mode: null,
    first_run_completed: false,
  };
  mockState.databaseStatus = {
    ...mockState.databaseStatus,
    mode: 'external',
    status: 'Connected',
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
    pgvector_available: true,
    migration_version: '0010_library_root_leases',
  };

  renderApp();

  const enterButton = await screen.findByRole('button', { name: '进入应用' });
  mockState.settings = {
    ...mockState.settings,
    database_mode: 'external',
    external_host: '192.168.31.25',
    external_port: 35973,
    external_database: 'image_db',
    external_username: 'helw',
    external_tls_mode: 'disable',
    first_run_completed: true,
  };

  fireEvent.click(enterButton);

  expect(await screen.findByRole('heading', { name: '工作台' })).toBeInTheDocument();
  expect(mockState.settings.database_mode).toBe('external');
  expect(mockState.settings.external_database).toBe('image_db');
});
