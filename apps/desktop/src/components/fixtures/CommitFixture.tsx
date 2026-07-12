import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { CommitProgress } from '../../lib/ipc/types';
import { CommitPage } from '../../pages/CommitPage';
import { Layout } from '../Layout';
import { importPlanFixture } from './importPlanFixture';

export type CommitFixtureState = 'confirm' | 'running' | 'success' | 'recovery';

const runningProgress: CommitProgress = {
  state: 'committing',
  import_run_id: importPlanFixture.import_run_id,
  current_stage: 'verifying',
  current_album: '城市建筑',
  albums_total: 6,
  albums_completed: 2,
  albums_skipped: 0,
  albums_failed: 0,
  images_committed: 226,
  errors: [],
};

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['database-info-dashboard'], {
  imports: { failed_album_count: 0, pending_review_count: 0, recovery_required_run_count: 0 },
});

function progressFor(state: CommitFixtureState): CommitProgress | null {
  if (state === 'running') return runningProgress;
  if (state === 'success') {
    return {
      ...runningProgress,
      state: 'completed',
      current_stage: 'done',
      current_album: null,
      albums_completed: 6,
      images_committed: 626,
    };
  }
  if (state === 'recovery') {
    return {
      ...runningProgress,
      state: 'recovery_required',
      current_stage: 'conflict',
      current_album: '花卉植物',
      albums_completed: 4,
      albums_failed: 1,
      images_committed: 426,
      errors: ['发布目录与 frozen plan 证据不一致；已停止自动处理。'],
    };
  }
  return null;
}

interface CommitFixtureProps {
  state?: CommitFixtureState;
}

export function CommitFixture({ state = 'confirm' }: CommitFixtureProps) {
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout currentRoute="commit" onNavigate={() => undefined} enablePolling={false}>
        <CommitPage
          initialPhase={
            state === 'confirm' ? 'confirm' : state === 'running' ? 'committing' : 'result'
          }
          initialPlan={state === 'confirm' ? importPlanFixture : null}
          initialProgress={progressFor(state)}
          initialImportRunId={importPlanFixture.import_run_id}
          enablePolling={false}
          onNavigate={() => undefined}
        />
      </Layout>
    </QueryClientProvider>
  );
}
