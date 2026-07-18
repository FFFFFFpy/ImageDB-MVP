import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { api } from '../lib/ipc/api';
import { importPlanFixture } from '../components/fixtures/importPlanFixture';
import type {
  ImportPlanImage,
  ReviewGroupDetail,
  ReviewGroupSummary,
  ReviewProgress,
} from '../lib/ipc/types';
import {
  groupImportPlanImagesByAlbum,
  invalidateReviewWorkflowQueries,
  ReviewPage,
  shouldIgnoreReviewShortcut,
  zoomViewAtPointer,
} from './ReviewPage';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const runId = '11111111-1111-1111-1111-111111111111';
const groupId = '22222222-2222-2222-2222-222222222222';

const groups: ReviewGroupSummary[] = [
  {
    group_id: groupId,
    state: 'pending',
    requires_manual_review: true,
    member_count: 4,
    import_member_count: 3,
    library_member_count: 1,
    kept_count: 4,
  },
];

const detail: ReviewGroupDetail = {
  group_id: groupId,
  state: 'pending',
  requires_manual_review: true,
  members: [
    {
      image_id: 'import-a',
      image_source: 'import',
      final_action: 'keep',
      decision_source: 'automatic',
      source_path: 'D:/source/a.jpg',
      relative_path: 'a.jpg',
      album_name: '来源图集',
      file_size: 100,
      width: 100,
      height: 80,
      format: 'jpeg',
    },
    {
      image_id: 'import-b',
      image_source: 'import',
      final_action: 'keep',
      decision_source: 'automatic',
      source_path: 'D:/source/b.jpg',
      relative_path: 'b.jpg',
      album_name: '来源图集',
      file_size: 110,
      width: 100,
      height: 80,
      format: 'jpeg',
    },
    {
      image_id: 'import-c',
      image_source: 'import',
      final_action: 'keep',
      decision_source: 'automatic',
      source_path: 'D:/source/c.jpg',
      relative_path: 'c.jpg',
      album_name: '另一个图集',
      file_size: 120,
      width: 100,
      height: 80,
      format: 'jpeg',
    },
    {
      image_id: 'library-a',
      image_source: 'library',
      final_action: 'keep',
      decision_source: 'automatic',
      source_path: 'D:/library/a.jpg',
      relative_path: 'Albums/existing/a.jpg',
      album_name: '库内图集',
      file_size: 100,
      width: 100,
      height: 80,
      format: 'jpeg',
    },
  ],
  evidence: [
    {
      candidate_id: 'edge-a',
      source_image_id: 'import-a',
      candidate_image_id: 'import-b',
      candidate_image_source: 'import',
      scope: 'cross_album',
      match_type: 'perceptual_near',
      blake3_equal: false,
      pixel_hash_equal: false,
      block_distance: 3,
      double_gradient_distance: 6,
      block_distance_ratio: 0.01,
      double_gradient_distance_ratio: 0.01,
      transform_type: 'identity',
      confidence: 0.96,
      automatic: false,
    },
  ],
};

function progress(overrides: Partial<ReviewProgress> = {}): ReviewProgress {
  return {
    import_run_id: runId,
    total_review_groups: 1,
    resolved_count: 0,
    remaining_count: 1,
    all_decided: false,
    ...overrides,
  };
}

function setupReviewMocks(
  groupDetail: ReviewGroupDetail = detail,
  groupSummaries: ReviewGroupSummary[] = groups,
  reviewProgress: ReviewProgress = progress(),
) {
  vi.spyOn(api, 'getReviewGroups').mockResolvedValue(groupSummaries);
  vi.spyOn(api, 'getReviewProgress').mockResolvedValue(reviewProgress);
  vi.spyOn(api, 'getReviewGroupDetail').mockResolvedValue(groupDetail);
  vi.spyOn(api, 'getFrozenImportPlanSummary').mockResolvedValue(null);
  vi.spyOn(api, 'getReviewGroupMemberPreview').mockResolvedValue({
    data_url: 'data:image/png;base64,AA==',
  });
  vi.spyOn(api, 'submitReviewGroupDecision').mockResolvedValue();
}

function renderReview(props: Partial<React.ComponentProps<typeof ReviewPage>> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <ReviewPage
        initialImportRunId={runId}
        enablePolling={false}
        onNavigate={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  );
}

describe('groupImportPlanImagesByAlbum', () => {
  test('groups images and preserves independent include decisions', () => {
    const images: ImportPlanImage[] = [
      {
        image_id: 'a',
        source_path: 'D:/a.jpg',
        relative_path: 'a.jpg',
        file_size: 10,
        album_name: 'A',
        album_id: 'album-a',
        source_album_id: 'album-a',
        included: true,
      },
      {
        image_id: 'b',
        source_path: 'D:/b.jpg',
        relative_path: 'b.jpg',
        file_size: 20,
        album_name: 'A',
        album_id: 'album-a',
        source_album_id: 'album-a',
        included: false,
      },
    ];
    expect(groupImportPlanImagesByAlbum(images)).toMatchObject([
      { albumId: 'album-a', imageCount: 1, skippedImageCount: 1, totalSize: 10 },
    ]);
  });
});

test('renders every group member and submits one action for every import member', async () => {
  setupReviewMocks();
  renderReview();

  await screen.findByText('4 张关联图片');
  expect(screen.getByText('Albums/existing/a.jpg')).toBeInTheDocument();
  const cards = document.querySelectorAll('.review-group-member');
  expect(cards).toHaveLength(4);

  fireEvent.click(within(cards[0] as HTMLElement).getByRole('button', { name: '排除' }));
  expect(cards[0]).toHaveClass('review-group-member--exclude');
  expect(within(cards[3] as HTMLElement).getByRole('button', { name: '排除' })).toBeDisabled();

  fireEvent.click(screen.getByRole('button', { name: '提交整组决定' }));
  await waitFor(() =>
    expect(api.submitReviewGroupDecision).toHaveBeenCalledWith(groupId, [
      { image_id: 'import-a', image_source: 'import', final_action: 'exclude' },
      { image_id: 'import-b', image_source: 'import', final_action: 'keep' },
      { image_id: 'import-c', image_source: 'import', final_action: 'keep' },
    ]),
  );
});

test('keeps every member decision readonly after a review group is resolved', async () => {
  const resolvedDetail = { ...detail, state: 'resolved' as const };
  const resolvedGroups = [{ ...groups[0], state: 'resolved' as const }];
  setupReviewMocks(
    resolvedDetail,
    resolvedGroups,
    progress({ resolved_count: 1, remaining_count: 0, all_decided: true }),
  );
  renderReview();

  expect(await screen.findByText('该审核组已经提交，成员决定为只读。')).toBeInTheDocument();
  const cards = document.querySelectorAll('.review-group-member');
  for (const card of cards) {
    expect(within(card as HTMLElement).getByRole('button', { name: '保留' })).toBeDisabled();
    expect(within(card as HTMLElement).getByRole('button', { name: '排除' })).toBeDisabled();
  }
  expect(screen.getByRole('button', { name: '提交整组决定' })).toBeDisabled();
});

test('shows frozen source mode as an explicit, default-off destructive toggle', async () => {
  const movedPlan = {
    ...importPlanFixture,
    source_file_mode: 'move_selected_without_backup' as const,
  };
  vi.spyOn(api, 'setImportPlanSourceFileMode').mockResolvedValue(movedPlan);
  renderReview({ initialPlan: importPlanFixture, initialShowPlan: true });

  const toggle = screen.getByRole('checkbox', { name: '移动已选源图片（无备份）' });
  expect(toggle).not.toBeChecked();
  fireEvent.click(toggle);
  await waitFor(() =>
    expect(api.setImportPlanSourceFileMode).toHaveBeenCalledWith(
      importPlanFixture.import_run_id,
      'move_selected_without_backup',
    ),
  );
  expect(await screen.findByText('不可撤销的源文件操作')).toBeInTheDocument();
});

test('invalidates group-level review workflow queries', async () => {
  const client = new QueryClient();
  const invalidate = vi.spyOn(client, 'invalidateQueries').mockResolvedValue();
  await invalidateReviewWorkflowQueries(client, runId);
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ['reviewGroups', runId] });
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ['reviewProgress', runId] });
});

test('shortcut guard ignores form editing and preview overlays', () => {
  const input = document.createElement('input');
  const inputEvent = new KeyboardEvent('keydown', { key: '1' });
  Object.defineProperty(inputEvent, 'target', { value: input });
  expect(shouldIgnoreReviewShortcut(inputEvent, false)).toBe(true);
  expect(shouldIgnoreReviewShortcut(new KeyboardEvent('keydown', { key: '1' }), true)).toBe(true);
});

test('pointer-centered zoom preserves the pointer anchor', () => {
  const next = zoomViewAtPointer(
    { scale: 1, offsetX: 0, offsetY: 0 },
    75,
    50,
    { left: 0, top: 0, width: 100, height: 100 },
    -1,
  );
  expect(next.scale).toBeCloseTo(1.1);
  expect(next.offsetX).toBeCloseTo(-2.5);
  expect(next.offsetY).toBeCloseTo(0);
});
