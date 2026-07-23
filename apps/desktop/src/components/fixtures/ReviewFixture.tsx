import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReviewGroupDetail, ReviewGroupSummary, ReviewProgress } from '../../lib/ipc/types';
import { PlanPage } from '../../pages/PlanPage';
import { ReviewPage } from '../../pages/ReviewPage';
import { Layout } from '../Layout';
import { importPlanFixture } from './importPlanFixture';

const importRunId = 'fixture-run-review';
const groupId = 'fixture-group-summer';

const groups: ReviewGroupSummary[] = [
  {
    group_id: groupId,
    state: 'pending',
    requires_manual_review: true,
    member_count: 3,
    import_member_count: 2,
    library_member_count: 1,
    kept_count: 3,
  },
];

const detail: ReviewGroupDetail = {
  group_id: groupId,
  state: 'pending',
  requires_manual_review: true,
  members: [
    {
      image_id: 'source-summer-01',
      image_source: 'import',
      final_action: 'keep',
      decision_source: 'automatic',
      source_path: 'D:/照片归档/2026 夏日旅行（待整理）/可爱宠物/IMG_0124.jpg',
      relative_path: '可爱宠物/IMG_0124.jpg',
      album_name: '可爱宠物',
      file_size: 2_731_482,
      width: 4480,
      height: 6720,
      format: 'JPEG',
    },
    {
      image_id: 'source-summer-02',
      image_source: 'import',
      final_action: 'keep',
      decision_source: 'automatic',
      source_path:
        'D:/照片归档/2026 夏日旅行（待整理）/可爱宠物/带有一段很长文件名用于响应式验证的 IMG_0124_副本.jpg',
      relative_path: '可爱宠物/带有一段很长文件名用于响应式验证的 IMG_0124_副本.jpg',
      album_name: '可爱宠物',
      file_size: 2_516_094,
      width: 4480,
      height: 6720,
      format: 'JPEG',
    },
    {
      image_id: 'library-summer-01',
      image_source: 'library',
      final_action: 'keep',
      decision_source: 'automatic',
      source_path: 'E:/ImageDB/图库/可爱宠物/IMG_0124_已入库.jpg',
      relative_path: '图库/可爱宠物/IMG_0124_已入库.jpg',
      album_name: '历史图库 · 可爱宠物',
      file_size: 2_731_482,
      width: 4480,
      height: 6720,
      format: 'JPEG',
    },
  ],
  evidence: [
    {
      candidate_id: 'fixture-candidate-summer',
      source_image_id: 'source-summer-01',
      candidate_image_id: 'source-summer-02',
      candidate_image_source: 'import',
      scope: 'intra_album',
      match_type: 'perceptual_similar',
      blake3_equal: false,
      pixel_hash_equal: false,
      block_distance: 6,
      double_gradient_distance: 9,
      block_distance_ratio: 6 / 256,
      double_gradient_distance_ratio: 9 / 544,
      transform_type: 'identity',
      confidence: 0.95,
      automatic: false,
    },
    {
      candidate_id: 'fixture-candidate-library',
      source_image_id: 'source-summer-01',
      candidate_image_id: 'library-summer-01',
      candidate_image_source: 'library',
      scope: 'library',
      match_type: 'pixel_exact',
      blake3_equal: false,
      pixel_hash_equal: true,
      block_distance: 0,
      double_gradient_distance: 0,
      block_distance_ratio: 0,
      double_gradient_distance_ratio: 0,
      transform_type: 'identity',
      confidence: 1,
      automatic: true,
    },
  ],
};

const progress: ReviewProgress = {
  import_run_id: importRunId,
  total_review_groups: 1,
  resolved_count: 0,
  remaining_count: 1,
  all_decided: false,
};

function imageDataUrl(variant: 'source' | 'candidate') {
  const dogX = variant === 'source' ? 385 : 415;
  const sunX = variant === 'source' ? 630 : 600;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="800" height="1200" viewBox="0 0 800 1200">
    <rect width="800" height="1200" fill="#bdd9e8"/>
    <circle cx="${sunX}" cy="145" r="70" fill="#f3c56a"/>
    <path d="M0 610 180 350 340 590 510 405 660 650 800 470V1200H0Z" fill="#607f70"/>
    <path d="M0 720 220 510 390 690 570 505 710 725 800 620V1200H0Z" fill="#789c78"/>
    <rect y="735" width="800" height="465" fill="#9dbc72"/>
    <ellipse cx="${dogX}" cy="1080" rx="190" ry="42" fill="#76905d"/>
    <ellipse cx="${dogX}" cy="885" rx="120" ry="145" fill="#c79456"/>
    <ellipse cx="${dogX - 106}" cy="840" rx="58" ry="104" fill="#8d623d" transform="rotate(18 ${dogX - 106} 840)"/>
    <ellipse cx="${dogX + 106}" cy="840" rx="58" ry="104" fill="#8d623d" transform="rotate(-18 ${dogX + 106} 840)"/>
    <ellipse cx="${dogX}" cy="910" rx="78" ry="62" fill="#e4bf88"/>
    <circle cx="${dogX - 43}" cy="855" r="13" fill="#263229"/>
    <circle cx="${dogX + 43}" cy="855" r="13" fill="#263229"/>
    <ellipse cx="${dogX}" cy="900" rx="22" ry="17" fill="#263229"/>
    <path d="M${dogX - 25} 930 Q${dogX} 950 ${dogX + 25} 930" fill="none" stroke="#4c382b" stroke-width="8" stroke-linecap="round"/>
    <rect x="${dogX - 92}" y="980" width="184" height="18" rx="9" fill="#167849"/>
  </svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['reviewGroups', importRunId], groups);
fixtureClient.setQueryData(['reviewProgress', importRunId], progress);
fixtureClient.setQueryData(['reviewGroupDetail', groupId], detail);
fixtureClient.setQueryData(['reviewFrozenImportPlanSummary', importRunId], null);
fixtureClient.setQueryData(
  ['import-runs-dashboard'],
  [
    {
      import_run_id: importRunId,
      source_root: 'D:/照片归档/2026 夏日旅行（待整理）',
      state: 'review_required',
      total_albums: 4,
      pending_albums: 0,
      analyzing_albums: 0,
      analyzed_albums: 3,
      review_required_albums: 1,
      failed_albums: 0,
      total_images: 128,
      pending_reviews: 1,
      duplicate_candidates: 2,
    },
  ],
);
for (const [index, member] of detail.members.entries()) {
  fixtureClient.setQueryData(
    ['reviewGroupMemberPreview', groupId, member.image_source, member.image_id],
    { data_url: imageDataUrl(index === 1 ? 'candidate' : 'source') },
  );
}
fixtureClient.setQueryData(['reviewProgress', importPlanFixture.import_run_id], {
  ...progress,
  import_run_id: importPlanFixture.import_run_id,
  remaining_count: 0,
  all_decided: true,
});
const draftPlanFixture = { ...importPlanFixture, plan_hash: null };
fixtureClient.setQueryData(
  ['reviewImportPlanDraftSummary', importPlanFixture.import_run_id],
  draftPlanFixture,
);
fixtureClient.setQueryData(['database-info-dashboard'], {
  imports: { failed_album_count: 0, pending_review_count: 32, recovery_required_run_count: 0 },
});

interface ReviewFixtureProps {
  view?: 'review' | 'plan';
}

export function ReviewFixture({ view = 'review' }: ReviewFixtureProps) {
  const showPlan = view === 'plan';
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout
        currentRoute={showPlan ? 'plan' : 'review'}
        onNavigate={() => undefined}
        enablePolling={false}
      >
        {showPlan ? (
          <PlanPage
            initialImportRunId={importPlanFixture.import_run_id}
            initialPlan={draftPlanFixture}
            onNavigate={() => undefined}
          />
        ) : (
          <ReviewPage
            initialImportRunId={importRunId}
            enablePolling={false}
            onNavigate={() => undefined}
          />
        )}
      </Layout>
    </QueryClientProvider>
  );
}
