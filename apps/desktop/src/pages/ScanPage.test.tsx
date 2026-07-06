import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { ImportAlbumStatus, ImportRunDashboard } from '../lib/ipc/types';
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
  retryImportAlbum: vi.fn((albumId: string) =>
    Promise.resolve({
      ...mockState.albums.find((album) => album.id === albumId)!,
      state: 'pending',
      last_error_message: null,
    }),
  ),
  getScanProgress: vi.fn(() =>
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

vi.mock('../lib/ipc/api', () => ({
  api: mockApi,
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

function renderScanPage(onNavigate = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return {
    onNavigate,
    ...render(
      <QueryClientProvider client={client}>
        <ScanPage onNavigate={onNavigate} />
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => {
  window.localStorage.clear();
  vi.clearAllMocks();
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
  test('loads album status from IPC and renders the workflow table', async () => {
    renderScanPage();

    expect(await screen.findByText('review-album')).toBeInTheDocument();
    expect(screen.getByText('failed-album')).toBeInTheDocument();
    expect(screen.getByText('done-album')).toBeInTheDocument();
    expect(screen.getByText('simulated failure')).toBeInTheDocument();
    expect(mockApi.getImportRunAlbums).toHaveBeenCalledWith('run-1');
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

  test('refetches album status after the page remounts', async () => {
    const first = renderScanPage();
    await screen.findByText('review-album');
    first.unmount();

    renderScanPage();
    await screen.findByText('review-album');
    expect(mockApi.getImportRunAlbums).toHaveBeenCalledTimes(2);
  });
});
