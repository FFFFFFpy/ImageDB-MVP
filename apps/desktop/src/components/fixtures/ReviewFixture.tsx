import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type {
  ReviewCandidateDetail,
  ReviewCandidateSummary,
  ReviewProgress,
} from '../../lib/ipc/types';
import { ReviewPage } from '../../pages/ReviewPage';
import { Layout } from '../Layout';
import { importPlanFixture } from './importPlanFixture';

const importRunId = 'fixture-run-review';
const candidateId = 'fixture-candidate-summer';

const queue: ReviewCandidateSummary[] = [
  {
    candidate_id: candidateId,
    source_image_id: 'source-summer-01',
    candidate_source_image_id: 'source-summer-02',
    candidate_library_image_id: null,
    scope: 'intra_album',
    match_type: 'perceptual_near',
    transform_type: 'identity',
    confidence: 0.92,
    album_name: '可爱宠物',
    has_decision: false,
  },
  {
    candidate_id: 'fixture-candidate-summer-2',
    source_image_id: 'source-summer-03',
    candidate_source_image_id: null,
    candidate_library_image_id: 'library-summer-01',
    scope: 'library',
    match_type: 'pixel_exact',
    transform_type: null,
    confidence: 1,
    album_name: '可爱宠物',
    has_decision: false,
  },
];

const detail: ReviewCandidateDetail = {
  candidate_id: candidateId,
  source_image_id: 'source-summer-01',
  source_image_path: 'D:/照片归档/2026 夏日旅行（待整理）/可爱宠物/IMG_0124.jpg',
  source_image_file_size: 2_731_482,
  source_image_width: 4032,
  source_image_height: 3024,
  candidate_source_image_id: 'source-summer-02',
  candidate_source_image_path: 'D:/照片归档/2026 夏日旅行（待整理）/可爱宠物/IMG_0124_1.jpg',
  candidate_source_image_file_size: 2_516_094,
  candidate_source_image_width: 4032,
  candidate_source_image_height: 3024,
  candidate_library_image_id: null,
  candidate_library_image_path: null,
  candidate_library_image_file_size: null,
  candidate_library_image_width: null,
  candidate_library_image_height: null,
  scope: 'intra_album',
  match_type: 'perceptual_near',
  blake3_equal: false,
  pixel_hash_equal: false,
  block_distance: 6,
  double_gradient_distance: 9,
  block_distance_ratio: 6 / 256,
  double_gradient_distance_ratio: 9 / 544,
  transform_type: 'identity',
  confidence: 0.92,
  album_name: '可爱宠物',
  album_id: 'album-pets',
  existing_decision: null,
};

const progress: ReviewProgress = {
  import_run_id: importRunId,
  total_review_candidates: 34,
  decided_count: 2,
  remaining_count: 32,
  all_decided: false,
};

function imageDataUrl(variant: 'source' | 'candidate') {
  const dogX = variant === 'source' ? 590 : 620;
  const sunX = variant === 'source' ? 930 : 900;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="800" viewBox="0 0 1200 800">
    <rect width="1200" height="800" fill="#bdd9e8"/>
    <circle cx="${sunX}" cy="135" r="70" fill="#f3c56a"/>
    <path d="M0 430 260 190 470 405 690 240 930 460 1200 250V800H0Z" fill="#607f70"/>
    <path d="M0 520 300 355 520 495 780 335 1010 510 1200 415V800H0Z" fill="#789c78"/>
    <rect y="535" width="1200" height="265" fill="#9dbc72"/>
    <ellipse cx="${dogX}" cy="690" rx="230" ry="42" fill="#76905d"/>
    <ellipse cx="${dogX}" cy="535" rx="120" ry="145" fill="#c79456"/>
    <ellipse cx="${dogX - 106}" cy="490" rx="58" ry="104" fill="#8d623d" transform="rotate(18 ${dogX - 106} 490)"/>
    <ellipse cx="${dogX + 106}" cy="490" rx="58" ry="104" fill="#8d623d" transform="rotate(-18 ${dogX + 106} 490)"/>
    <ellipse cx="${dogX}" cy="560" rx="78" ry="62" fill="#e4bf88"/>
    <circle cx="${dogX - 43}" cy="505" r="13" fill="#263229"/>
    <circle cx="${dogX + 43}" cy="505" r="13" fill="#263229"/>
    <ellipse cx="${dogX}" cy="550" rx="22" ry="17" fill="#263229"/>
    <path d="M${dogX - 25} 580 Q${dogX} 600 ${dogX + 25} 580" fill="none" stroke="#4c382b" stroke-width="8" stroke-linecap="round"/>
    <rect x="${dogX - 92}" y="630" width="184" height="18" rx="9" fill="#167849"/>
  </svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['reviewQueue', importRunId], queue);
fixtureClient.setQueryData(['reviewProgress', importRunId], progress);
fixtureClient.setQueryData(['reviewDetail', candidateId], detail);
fixtureClient.setQueryData(['reviewFrozenImportPlanSummary', importRunId], null);
fixtureClient.setQueryData(['reviewQueue', importPlanFixture.import_run_id], []);
fixtureClient.setQueryData(['reviewProgress', importPlanFixture.import_run_id], {
  ...progress,
  import_run_id: importPlanFixture.import_run_id,
  decided_count: 34,
  remaining_count: 0,
  all_decided: true,
});
fixtureClient.setQueryData(
  ['reviewFrozenImportPlanSummary', importPlanFixture.import_run_id],
  importPlanFixture,
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
      <Layout currentRoute="review" onNavigate={() => undefined} enablePolling={false}>
        <ReviewPage
          initialImportRunId={showPlan ? importPlanFixture.import_run_id : importRunId}
          initialPreviews={{ left: imageDataUrl('source'), right: imageDataUrl('candidate') }}
          initialPlan={showPlan ? importPlanFixture : null}
          initialShowPlan={showPlan}
          enablePolling={false}
          onNavigate={() => undefined}
        />
      </Layout>
    </QueryClientProvider>
  );
}
