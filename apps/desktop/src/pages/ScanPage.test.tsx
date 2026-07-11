import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { ImportAlbumStatus, ImportRunDashboard, ScanProgress } from '../lib/ipc/types';
import {
  isTerminalScanState,
  nextActionLabelForScanState,
  nextRouteForScanState,
  ScanPage,
} from './ScanPage';

const mockState = vi.hoisted(() => ({
  dashboard: [
    {
      import_run_id: 'run-1',
      source_root: 'D:/Photos',
      state: 'review_required',
      total_albums: 3,
      pending_albums: 1,
      analyzing_albums: 0,
      analyzed_albums: 1,
      review_required_albums: 1,
      failed_albums: 1,
      total_images: 12,
      pending_reviews: 2,
      duplicate_candidates: 3,
    },
  ] as ImportRunDashboard[],
  albums: [
    {
      id: 'album-review',
      import_run_id: 'run-1',
      source_name: 'review-album',
      source_path: 'D:/Photos/review',
      state: 'review_required',
      image_count: 5,
      fingerprinted_count: 5,
      duplicate_candidate_count: 2,
      review_candidate_count: 2,
      last_error_message: null,
      analysis_started_at: null,
      analysis_completed_at: null,
    },
    {
      id: 'album-failed',
      import_run_id: 'run-1',
      source_name: 'failed-album',
      source_path: 'D:/Photos/failed',
      state: 'failed',
      image_count: 1,
      fingerprinted_count: 0,
      duplicate_candidate_count: 0,
      review_candidate_count: 0,
      last_error_message: 'simulated failure',
      analysis_started_at: null,
      analysis_completed_at: null,
    },
    {
      id: 'album-done',
      import_run_id: 'run-1',
      source_name: 'done-album',
      source_path: 'D:/Photos/done',
      state: 'analyzed',
      image_count: 6,
      fingerprinted_count: 6,
      duplicate_candidate_count: 1,
      review_candidate_count: 0,
      last_error_message: null,
      analysis_started_at: null,
      analysis_completed_at: null,
    },
  ] as ImportAlbumStatus[],
}));

const mockApi = vi.hoisted(() => ({
  getImportRunsDashboard: vi.fn(() => Promise.resolve(mockState.dashboard)),
  getImportRunAlbums: vi.fn(() => Promise.resolve(mockState.albums)),
  resumeImportRun: vi.fn(() => Promise.resolve('scan started')),
  retryImportAlbum: vi.fn((albumId: string) =>
    Promise.resolve({
      ...mockState.albums.find((album) => album.id === albumId)!,
      state: 'pending',
      last_error_message: null,
    }),
  ),
  abandonImportRun: vi.fn(() => Promise.resolve()),
  getScanProgress: vi.fn((): Promise<ScanProgress> =>
    Promise.resolve({
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
    }),
  ),
  validateSourceDirectory: vi.fn(),
  startScan: vi.fn(),
  cancelScan: vi.fn(),
}));

const mockListen = vi.hoisted(() => vi.fn(() => Promise.resolve(() => undefined)));

vi.mock('../lib/ipc/api', () => ({
  api: mockApi,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: mockListen,
}));

function renderScanPage(onNavigate = vi.fn(), initialImportRunId: string | null = 'run-1') {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return {
    client,
    onNavigate,
    ...render(
      <QueryClientProvider client={client}>
        <ScanPage initialImportRunId={initialImportRunId} onNavigate={onNavigate} />
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => {
  window.localStorage.clear();
  vi.clearAllMocks();
  mockListen.mockImplementation(() => Promise.resolve(() => undefined));
});

afterEach(() => {
  cleanup();
});

describe('ScanPage state routing', () => {
  test('treats committable and review scan states as terminal', () => {
    expect(isTerminalScanState('ready_to_commit')).toBe(true);
    expect(isTerminalScanState('review_required')).toBe(true);
    expect(isTerminalScanState('completed')).toBe(true);
    expect(isTerminalScanState('cancelled')).toBe(true);
    expect(isTerminalScanState('failed')).toBe(true);
    expect(isTerminalScanState('running')).toBe(false);
  });

  test('routes completed scan states to the next public workflow page', () => {
    expect(nextRouteForScanState('review_required')).toBe('review');
    expect(nextRouteForScanState('ready_to_commit')).toBe('review');
    expect(nextRouteForScanState('failed')).toBeNull();
    expect(nextRouteForScanState('cancelled')).toBeNull();
    expect(nextRouteForScanState('completed')).toBeNull();
    expect(nextRouteForScanState(null)).toBeNull();
  });

  test('uses the same review action for review-required and ready-to-commit results', () => {
    expect(nextActionLabelForScanState('ready_to_commit')).toBe(
      nextActionLabelForScanState('review_required'),
    );
    expect(nextActionLabelForScanState('ready_to_commit')).not.toBeNull();
    expect(nextActionLabelForScanState('failed')).toBeNull();
  });
});

describe('ScanPage album workflow', () => {
  test('keeps an explicitly selected run instead of restoring an older global tracker run', async () => {
    mockApi.getScanProgress.mockResolvedValueOnce({
      state: 'cancelled',
      import_run_id: 'run-old',
      current_stage: 'cancelled',
      current_album: null,
      processed_images: 4,
      total_albums: 1,
      total_images: 4,
      duplicate_count: 0,
      error_count: 0,
      errors: [],
    });

    renderScanPage(vi.fn(), 'run-1');

    expect(await screen.findByText('review-album')).toBeInTheDocument();
    expect(mockApi.getImportRunAlbums).toHaveBeenCalledWith('run-1');
    expect(mockApi.getScanProgress).toHaveBeenCalledTimes(1);
    expect(mockApi.getImportRunAlbums).not.toHaveBeenCalledWith('run-old');
  });

  test('does not show stale latest-run albums until a run is active', async () => {
    renderScanPage(vi.fn(), null);

    expect(await screen.findByText('暂无图集状态。验证源目录后开始分析。')).toBeInTheDocument();
    expect(screen.queryByText('review-album')).not.toBeInTheDocument();
    expect(mockApi.getImportRunAlbums).not.toHaveBeenCalled();
  });

  test('loads album status from IPC and renders the workflow table', async () => {
    renderScanPage();

    expect(await screen.findByText('review-album')).toBeInTheDocument();
    expect(screen.getByText('failed-album')).toBeInTheDocument();
    expect(screen.getByText('done-album')).toBeInTheDocument();
    expect(screen.getByText('simulated failure')).toBeInTheDocument();
    expect(mockApi.getImportRunAlbums).toHaveBeenCalledWith('run-1');
  });

  test('clears the active run when the source path is edited', async () => {
    renderScanPage();

    expect(await screen.findByText('review-album')).toBeInTheDocument();
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'D:/Other' } });

    expect(screen.queryByText('review-album')).not.toBeInTheDocument();
    expect(screen.getByText('暂无图集状态。验证源目录后开始分析。')).toBeInTheDocument();
  });

  test('continues the active run through resumeImportRun', async () => {
    renderScanPage();

    fireEvent.click(await screen.findByRole('button', { name: '继续分析' }));
    await waitFor(() => expect(mockApi.resumeImportRun).toHaveBeenCalledWith('run-1'));
    expect(mockApi.startScan).not.toHaveBeenCalled();
  });

  test('shows abandoned history without resume or retry workflow controls', async () => {
    const original = mockState.dashboard;
    mockState.dashboard = original.map((run) =>
      run.import_run_id === 'run-1'
        ? { ...run, state: 'abandoned', pending_albums: 1, failed_albums: 1 }
        : run,
    );
    try {
      renderScanPage();
      expect(await screen.findByText('review-album')).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: '继续分析' })).not.toBeInTheDocument();
      expect(
        screen.queryByRole('button', { name: '放弃旧 checkpoint，重新分析' }),
      ).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: '重试' })).not.toBeInTheDocument();
    } finally {
      mockState.dashboard = original;
    }
  });

  test('abandons an old checkpoint and starts a clean run for the same source', async () => {
    const original = mockState.dashboard;
    mockState.dashboard = [{ ...original[0], state: 'failed', pending_albums: 0 }];
    mockApi.validateSourceDirectory.mockResolvedValueOnce({
      path: 'D:/Photos',
      albums: ['album-a'],
      album_count: 1,
    });
    mockApi.startScan.mockResolvedValueOnce('scan started');
    try {
      renderScanPage();
      fireEvent.click(await screen.findByRole('button', { name: '放弃旧 checkpoint，重新分析' }));
      await waitFor(() => expect(mockApi.abandonImportRun).toHaveBeenCalledWith('run-1'));
      expect(mockApi.validateSourceDirectory).toHaveBeenCalledWith('D:/Photos');
      await waitFor(() => expect(mockApi.startScan).toHaveBeenCalledWith('D:/Photos'));
      expect(mockApi.resumeImportRun).not.toHaveBeenCalled();
    } finally {
      mockState.dashboard = original;
    }
  });

  test('clears stale terminal controls immediately when resuming a run', async () => {
    mockApi.getScanProgress.mockResolvedValueOnce({
      state: 'cancelled',
      import_run_id: 'run-1',
      current_stage: 'cancelled',
      current_album: null,
      processed_images: 6,
      total_albums: 3,
      total_images: 12,
      duplicate_count: 2,
      error_count: 0,
      errors: [],
    });

    renderScanPage(vi.fn(), 'run-1');

    expect(await screen.findByRole('button', { name: '重置' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '继续分析' }));

    await waitFor(() => expect(mockApi.resumeImportRun).toHaveBeenCalledWith('run-1'));
    expect(screen.queryByRole('button', { name: '重置' })).not.toBeInTheDocument();
  });

  test('shows retry for failed albums and review only for albums with candidates', async () => {
    const { onNavigate } = renderScanPage();

    const failedRow = (await screen.findByText('failed-album')).closest('tr');
    expect(failedRow).not.toBeNull();
    fireEvent.click(within(failedRow!).getByRole('button'));
    await waitFor(() => expect(mockApi.retryImportAlbum).toHaveBeenCalled());
    expect(mockApi.retryImportAlbum.mock.calls[0][0]).toBe('album-failed');

    const reviewRow = screen.getByText('review-album').closest('tr');
    expect(reviewRow).not.toBeNull();
    fireEvent.click(within(reviewRow!).getByRole('button'));
    expect(onNavigate).toHaveBeenCalledWith('review');
    expect(
      within(screen.getByText('done-album').closest('tr')!).queryAllByRole('button'),
    ).toHaveLength(0);
  });

  test('shows the active controls and disables retry while the selected run is scanning', async () => {
    mockApi.getScanProgress.mockResolvedValueOnce({
      state: 'running',
      import_run_id: 'run-1',
      current_stage: 'analyzing',
      current_album: 'done-album',
      processed_images: 2,
      total_albums: 3,
      total_images: 12,
      duplicate_count: 0,
      error_count: 0,
      errors: [],
    });

    renderScanPage(vi.fn(), 'run-1');

    const failedRow = (await screen.findByText('failed-album')).closest('tr');
    expect(failedRow).not.toBeNull();
    expect(within(failedRow!).getByRole('button', { name: '重试' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '取消扫描' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '继续分析' })).not.toBeInTheDocument();
    expect(mockApi.retryImportAlbum).not.toHaveBeenCalled();
  });

  test('keeps the selected run visible but blocks its actions while another run is scanning', async () => {
    mockApi.getScanProgress.mockResolvedValueOnce({
      state: 'running',
      import_run_id: 'run-other',
      current_stage: 'analyzing',
      current_album: 'other-album',
      processed_images: 2,
      total_albums: 4,
      total_images: 20,
      duplicate_count: 0,
      error_count: 0,
      errors: [],
    });

    renderScanPage(vi.fn(), 'run-1');

    const failedRow = (await screen.findByText('failed-album')).closest('tr');
    expect(failedRow).not.toBeNull();
    expect(mockApi.getImportRunAlbums).toHaveBeenCalledWith('run-1');
    expect(screen.getByText(/另一个分析任务正在运行（run-other）/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '继续分析' })).toBeDisabled();
    expect(within(failedRow!).getByRole('button', { name: '重试' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: '取消扫描' })).not.toBeInTheDocument();
  });

  test('clears the other-run lock when the global tracker returns to idle', async () => {
    let pollScanProgress: (() => Promise<void>) | null = null;
    const nativeSetInterval = globalThis.setInterval;
    const intervalSpy = vi.spyOn(globalThis, 'setInterval').mockImplementation(((
      handler: TimerHandler,
      delay?: number,
      ...args: unknown[]
    ) => {
      if (delay === 2000) {
        pollScanProgress = handler as () => Promise<void>;
        return 123 as unknown as ReturnType<typeof setInterval>;
      }
      return nativeSetInterval(handler, delay, ...args);
    }) as typeof setInterval);
    mockApi.getScanProgress
      .mockResolvedValueOnce({
        state: 'running',
        import_run_id: 'run-other',
        current_stage: 'analyzing',
        current_album: 'other-album',
        processed_images: 2,
        total_albums: 4,
        total_images: 20,
        duplicate_count: 0,
        error_count: 0,
        errors: [],
      })
      .mockResolvedValueOnce({
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

    try {
      renderScanPage(vi.fn(), 'run-1');

      const failedRow = (await screen.findByText('failed-album')).closest('tr');
      expect(failedRow).not.toBeNull();
      expect(screen.getByText(/另一个分析任务正在运行（run-other）/)).toBeInTheDocument();
      await waitFor(() => expect(pollScanProgress).not.toBeNull());

      await act(async () => {
        await pollScanProgress!();
      });

      await waitFor(() =>
        expect(screen.queryByText(/另一个分析任务正在运行/)).not.toBeInTheDocument(),
      );
      expect(screen.getByRole('button', { name: '继续分析' })).toBeEnabled();
      expect(within(failedRow!).getByRole('button', { name: '重试' })).toBeEnabled();
    } finally {
      intervalSpy.mockRestore();
    }
  });

  test('recovers the resume controls when scan event listener registration fails', async () => {
    mockListen.mockRejectedValueOnce(new Error('listen failed'));
    renderScanPage();

    fireEvent.click(await screen.findByRole('button', { name: '继续分析' }));

    expect(await screen.findByText('Error: listen failed')).toBeInTheDocument();
    expect(mockApi.resumeImportRun).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: '继续分析' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: '取消扫描' })).not.toBeInTheDocument();
  });

  test('shows an album query failure instead of presenting it as an empty result', async () => {
    mockApi.getImportRunAlbums.mockRejectedValueOnce(new Error('albums unavailable'));
    renderScanPage();

    expect(
      await screen.findByText('加载图集状态失败：Error: albums unavailable'),
    ).toBeInTheDocument();
    expect(screen.getByText('图集状态加载失败，请稍后重试。')).toBeInTheDocument();
    expect(screen.queryByText('暂无图集状态。验证源目录后开始分析。')).not.toBeInTheDocument();
  });

  test('shows a run query failure instead of presenting it as an empty result', async () => {
    mockApi.getImportRunsDashboard.mockRejectedValueOnce(new Error('runs unavailable'));
    renderScanPage(vi.fn(), null);

    expect(
      await screen.findByText('加载导入任务失败：Error: runs unavailable'),
    ).toBeInTheDocument();
    expect(screen.getByText('导入任务加载失败，请稍后重试。')).toBeInTheDocument();
    expect(screen.queryByText('暂无图集状态。验证源目录后开始分析。')).not.toBeInTheDocument();
  });

  test('does not reuse validated source details from an older draft for a selected run', async () => {
    window.localStorage.setItem(
      'imagedb.scan.draft',
      JSON.stringify({
        sourcePath: 'D:/Old',
        sourceInfo: {
          path: 'D:/Old',
          albums: ['old-draft-album'],
          album_count: 1,
        },
      }),
    );

    renderScanPage(vi.fn(), 'run-1');

    expect(await screen.findByText('review-album')).toBeInTheDocument();
    expect(screen.getByRole('textbox')).toHaveValue('D:/Photos');
    expect(screen.queryByText(/old-draft-album/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '开始分析' })).not.toBeInTheDocument();
  });

  test('ignores a late validation result after a dashboard run is selected', async () => {
    let resolveValidation!: (value: {
      path: string;
      albums: string[];
      album_count: number;
    }) => void;
    mockApi.validateSourceDirectory.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveValidation = resolve;
        }),
    );
    window.localStorage.setItem(
      'imagedb.scan.draft',
      JSON.stringify({ sourcePath: 'D:/Old', sourceInfo: null }),
    );

    const view = renderScanPage(vi.fn(), null);
    fireEvent.click(await screen.findByRole('button', { name: '验证' }));
    expect(screen.getByRole('textbox')).toBeDisabled();

    view.rerender(
      <QueryClientProvider client={view.client}>
        <ScanPage initialImportRunId="run-1" onNavigate={view.onNavigate} />
      </QueryClientProvider>,
    );
    expect(await screen.findByDisplayValue('D:/Photos')).toBeInTheDocument();

    resolveValidation({ path: 'D:/Old', albums: ['late-old-album'], album_count: 1 });

    await waitFor(() => expect(screen.getByRole('textbox')).toHaveValue('D:/Photos'));
    expect(screen.queryByText(/late-old-album/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '开始分析' })).not.toBeInTheDocument();
  });

  test('shows retry failures instead of silently ignoring them', async () => {
    mockApi.retryImportAlbum.mockRejectedValueOnce(new Error('retry failed'));
    renderScanPage();

    const failedRow = (await screen.findByText('failed-album')).closest('tr');
    expect(failedRow).not.toBeNull();
    fireEvent.click(within(failedRow!).getByRole('button', { name: '重试' }));

    expect(await screen.findByText('重试失败：Error: retry failed')).toBeInTheDocument();
  });

  test('refetches album status after the page remounts', async () => {
    const first = renderScanPage();
    await screen.findByText('review-album');
    first.unmount();

    renderScanPage();
    await screen.findByText('review-album');
    expect(mockApi.getImportRunAlbums).toHaveBeenCalledTimes(2);
  });
});
