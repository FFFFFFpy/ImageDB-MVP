import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ImportAlbumStatus, ImportRunDashboard, ScanProgress } from '../../lib/ipc/types';
import { ScanPage } from '../../pages/ScanPage';
import { Layout } from '../Layout';

const importRun: ImportRunDashboard = {
  import_run_id: 'fixture-run-scan',
  source_root: 'D:/照片归档/2026 夏日旅行（待整理）',
  state: 'analyzing',
  total_albums: 6,
  pending_albums: 2,
  analyzing_albums: 1,
  analyzed_albums: 2,
  review_required_albums: 1,
  failed_albums: 0,
  total_images: 808,
  pending_reviews: 12,
  duplicate_candidates: 28,
};

const albums: ImportAlbumStatus[] = [
  ['album-1', '旅行风光', 'analyzed', 1254, 28, 0, null],
  ['album-2', '城市建筑', 'analyzing', 1128, 16, 0, null],
  ['album-3', '可爱宠物', 'review_required', 1034, 34, 12, null],
  ['album-4', '生活记录', 'pending', 862, 0, 0, null],
  ['album-5', '花卉植物', 'pending', 931, 0, 0, null],
  ['album-6', '损坏文件（需检查）', 'failed', 24, 0, 0, '无法解码 2 张图片；其余文件未修改。'],
].map(([id, name, state, imageCount, duplicates, reviews, error]) => ({
  id: String(id),
  import_run_id: importRun.import_run_id,
  source_name: String(name),
  source_path: `${importRun.source_root}/${String(name)}`,
  state: state as ImportAlbumStatus['state'],
  image_count: Number(imageCount),
  fingerprinted_count: state === 'pending' ? 0 : Number(imageCount),
  duplicate_candidate_count: Number(duplicates),
  review_candidate_count: Number(reviews),
  last_error_message: error ? String(error) : null,
  analysis_started_at: null,
  analysis_completed_at: null,
}));

const progress: ScanProgress = {
  state: 'analyzing',
  import_run_id: importRun.import_run_id,
  current_stage: 'fingerprinting',
  current_album: '城市建筑',
  processed_images: 438,
  total_albums: 6,
  total_images: 808,
  duplicate_count: 28,
  error_count: 0,
  errors: [],
};

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['import-runs-dashboard'], [importRun]);
fixtureClient.setQueryData(['import-run-albums', importRun.import_run_id], albums);
fixtureClient.setQueryData(['database-info-dashboard'], {
  imports: { failed_album_count: 0, pending_review_count: 12, recovery_required_run_count: 0 },
});

export function ScanFixture() {
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout currentRoute="scan" onNavigate={() => undefined} enablePolling={false}>
        <ScanPage
          initialImportRunId={importRun.import_run_id}
          initialProgress={progress}
          enablePolling={false}
          onNavigate={() => undefined}
        />
      </Layout>
    </QueryClientProvider>
  );
}
