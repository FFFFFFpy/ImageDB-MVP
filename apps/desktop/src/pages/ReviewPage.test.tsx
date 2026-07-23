import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { api } from '../lib/ipc/api';
import { importPlanFixture } from '../components/fixtures/importPlanFixture';
import type {
  ReviewGroupDetail,
  ReviewGroupSummary,
  ReviewProgress,
} from '../lib/ipc/types';
import {
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
      width: 4480,
      height: 6720,
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
  vi.spyOn(api, 'getImportRunsDashboard').mockResolvedValue([
    {
      import_run_id: runId,
      source_root: 'D:/source',
      state: 'review_required',
      total_albums: 2,
      pending_albums: 0,
      analyzing_albums: 0,
      analyzed_albums: 1,
      review_required_albums: 1,
      failed_albums: 0,
      total_images: 4,
      pending_reviews: reviewProgress.remaining_count,
      duplicate_candidates: 1,
    },
  ]);
  vi.spyOn(api, 'getReviewGroups').mockResolvedValue(groupSummaries);
  vi.spyOn(api, 'getReviewProgress').mockResolvedValue(reviewProgress);
  vi.spyOn(api, 'getReviewGroupDetail').mockResolvedValue(groupDetail);
  vi.spyOn(api, 'getFrozenImportPlanSummary').mockResolvedValue(null);
  vi.spyOn(api, 'getImportPlanDraftSummary').mockResolvedValue(null);
  vi.spyOn(api, 'getReviewGroupMemberPreview').mockResolvedValue({
    data_url: 'data:image/png;base64,AA==',
  });
  vi.spyOn(api, 'submitReviewGroupDecision').mockResolvedValue();
}

function renderReview(props: Partial<React.ComponentProps<typeof ReviewPage>> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const result = render(
    <QueryClientProvider client={client}>
      <ReviewPage
        initialImportRunId={runId}
        enablePolling={false}
        onNavigate={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  );
  return { ...result, client };
}

test('renders every group member and submits one action for every import member', async () => {
  setupReviewMocks();
  renderReview();

  await screen.findByText('4 张关联图片');
  expect(screen.getByTitle('Albums/existing/a.jpg')).toBeInTheDocument();
  const groupHeading = document.querySelector('.review-group-heading') as HTMLElement;
  expect(within(groupHeading).getByRole('button', { name: '保存整组决定' })).toBeInTheDocument();
  expect(within(groupHeading).getByText('保留 4 张 · 排除 0 张')).toBeInTheDocument();
  const cards = document.querySelectorAll('.review-group-member');
  expect(cards).toHaveLength(4);
  expect(within(cards[0] as HTMLElement).getByRole('button', { name: '查看 a.jpg' })).toHaveStyle({
    aspectRatio: '4480 / 6720',
  });

  fireEvent.click(within(cards[0] as HTMLElement).getByRole('button', { name: '排除' }));
  expect(cards[0]).toHaveClass('review-group-member--exclude');
  expect(within(cards[3] as HTMLElement).getByRole('button', { name: '排除' })).toBeDisabled();

  fireEvent.click(screen.getByRole('button', { name: '保存整组决定' }));
  await waitFor(() =>
    expect(api.submitReviewGroupDecision).toHaveBeenCalledWith(groupId, [
      { image_id: 'import-a', image_source: 'import', final_action: 'exclude' },
      { image_id: 'import-b', image_source: 'import', final_action: 'keep' },
      { image_id: 'import-c', image_source: 'import', final_action: 'keep' },
    ]),
  );
});

test('restores complete image metadata and fingerprint evidence for group review', async () => {
  setupReviewMocks();
  renderReview();

  await screen.findByText('4 张关联图片');
  const cards = document.querySelectorAll('.review-group-member');
  fireEvent.click(within(cards[0] as HTMLElement).getByText('查看完整图片信息'));
  expect(within(cards[0] as HTMLElement).getByText('D:/source/a.jpg')).toBeInTheDocument();
  expect(within(cards[0] as HTMLElement).getByText('import-a')).toBeInTheDocument();
  expect(within(cards[0] as HTMLElement).getByText('系统默认')).toBeInTheDocument();

  const evidenceSummary = screen.getByText('感知近似 · 最高相似度 96.0% · 1 条边');
  const evidenceDetails = evidenceSummary.closest('details');
  expect(evidenceDetails).not.toHaveAttribute('open');
  expect(screen.queryByText('跨图集')).not.toBeVisible();
  fireEvent.click(evidenceSummary.closest('summary') as HTMLElement);

  expect(screen.getByText('感知近似')).toBeInTheDocument();
  expect(screen.getByText('跨图集')).toBeInTheDocument();
  expect(screen.getByText('需人工审核')).toBeInTheDocument();
  expect(screen.getByText('96.0%')).toBeInTheDocument();
  expect(screen.getByText('原方向')).toBeInTheDocument();
  expect(screen.getByText('3 / 256（差异 1.0%）')).toBeInTheDocument();
  expect(screen.getByText('6 / 544（差异 1.0%）')).toBeInTheDocument();
  expect(screen.getByText('edge-a')).toBeInTheDocument();
});

test('allows a resolved group draft to be adjusted until the plan is frozen', async () => {
  const resolvedDetail = { ...detail, state: 'resolved' as const };
  const resolvedGroups = [{ ...groups[0], state: 'resolved' as const }];
  setupReviewMocks(
    resolvedDetail,
    resolvedGroups,
    progress({ resolved_count: 1, remaining_count: 0, all_decided: true }),
  );
  renderReview();

  expect(
    await screen.findByText('该审核组已有已保存草稿；冻结导入计划前仍可继续调整。'),
  ).toBeInTheDocument();
  const groupHeading = document.querySelector('.review-group-heading') as HTMLElement;
  expect(within(groupHeading).getByText('草稿已保存')).toBeInTheDocument();
  const cards = document.querySelectorAll('.review-group-member');
  for (const card of Array.from(cards).slice(0, 3)) {
    expect(within(card as HTMLElement).getByRole('button', { name: '保留' })).toBeEnabled();
    expect(within(card as HTMLElement).getByRole('button', { name: '排除' })).toBeEnabled();
  }
  fireEvent.click(within(cards[0] as HTMLElement).getByRole('button', { name: '排除' }));
  expect(within(groupHeading).getByText('有未保存修改')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '更新整组决定' }));
  await waitFor(() => expect(api.submitReviewGroupDecision).toHaveBeenCalledTimes(1));
});

test('keeps resolved answers as drafts while analysis is still incomplete', async () => {
  const resolvedDetail = { ...detail, state: 'resolved' as const };
  const resolvedGroups = [{ ...groups[0], state: 'resolved' as const }];
  setupReviewMocks(
    resolvedDetail,
    resolvedGroups,
    progress({ resolved_count: 1, remaining_count: 0, all_decided: true }),
  );
  vi.mocked(api.getImportRunsDashboard).mockResolvedValue([
    {
      import_run_id: runId,
      source_root: 'D:/source',
      state: 'analyzing',
      total_albums: 2,
      pending_albums: 1,
      analyzing_albums: 0,
      analyzed_albums: 0,
      review_required_albums: 1,
      failed_albums: 0,
      total_images: 4,
      pending_reviews: 0,
      duplicate_candidates: 1,
    },
  ]);
  const onNavigate = vi.fn();
  renderReview({ onNavigate });

  expect(await screen.findByText('当前审核答案已保存，分析尚未完成')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: '生成人工复核入库计划' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: '继续分析' }));
  expect(onNavigate).toHaveBeenCalledWith('scan');
});

test('refreshes review groups once when an analyzing run becomes reviewable', async () => {
  setupReviewMocks();
  const analyzingRun = {
    import_run_id: runId,
    source_root: 'D:/source',
    state: 'analyzing',
    total_albums: 2,
    pending_albums: 1,
    analyzing_albums: 0,
    analyzed_albums: 0,
    review_required_albums: 1,
    failed_albums: 0,
    total_images: 4,
    pending_reviews: 1,
    duplicate_candidates: 1,
  };
  const completedRun = {
    ...analyzingRun,
    state: 'review_required',
    pending_albums: 0,
    analyzed_albums: 1,
  };
  const newGroup = {
    ...groups[0],
    group_id: '33333333-3333-3333-3333-333333333333',
    member_count: 2,
    import_member_count: 2,
    kept_count: 2,
  };
  vi.mocked(api.getImportRunsDashboard).mockResolvedValue([analyzingRun]);
  vi.mocked(api.getReviewGroups)
    .mockResolvedValueOnce(groups)
    .mockResolvedValue([groups[0], newGroup]);

  const { client } = renderReview();

  expect(await screen.findByRole('button', { name: /组 1/ })).toBeVisible();
  await waitFor(() => expect(api.getReviewGroups).toHaveBeenCalledTimes(1));
  await waitFor(() =>
    expect(client.getQueryData(['import-runs-dashboard'])).toEqual([analyzingRun]),
  );

  act(() => client.setQueryData(['import-runs-dashboard'], [completedRun]));

  expect(await screen.findByRole('button', { name: /组 2/ })).toBeVisible();
  await waitFor(() => expect(api.getReviewGroups).toHaveBeenCalledTimes(2));
  expect(api.getReviewProgress).toHaveBeenCalledTimes(2);
  expect(api.getReviewGroupDetail).toHaveBeenCalledTimes(2);
});

test('generates an editable draft before the plan can be locked', async () => {
  const resolvedDetail = { ...detail, state: 'resolved' as const };
  const resolvedGroups = [{ ...groups[0], state: 'resolved' as const }];
  setupReviewMocks(
    resolvedDetail,
    resolvedGroups,
    progress({ resolved_count: 1, remaining_count: 0, all_decided: true }),
  );
  const draftPlan = { ...importPlanFixture, plan_hash: null };
  vi.spyOn(api, 'generateImportPlan').mockResolvedValue(draftPlan);
  const onGoPlan = vi.fn();
  renderReview({ onGoPlan });

  fireEvent.click(await screen.findByRole('button', { name: '生成人工复核入库计划' }));

  await waitFor(() =>
    expect(api.generateImportPlan).toHaveBeenCalledWith(runId),
  );
  expect(onGoPlan).toHaveBeenCalledWith(draftPlan.import_run_id);
  expect(draftPlan.plan_hash).toBeNull();
});

test('invalidates group-level review workflow queries', async () => {
  const client = new QueryClient();
  const invalidate = vi.spyOn(client, 'invalidateQueries').mockResolvedValue();
  await invalidateReviewWorkflowQueries(client, runId);
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ['reviewGroups', runId] });
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ['reviewProgress', runId] });
  expect(invalidate).toHaveBeenCalledWith({
    queryKey: ['reviewImportPlanDraftSummary', runId],
  });
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
