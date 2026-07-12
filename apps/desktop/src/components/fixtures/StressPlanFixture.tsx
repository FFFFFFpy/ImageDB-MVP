import { useMemo } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReviewProgress } from '../../lib/ipc/types';
import { ReviewPage } from '../../pages/ReviewPage';
import { Layout } from '../Layout';
import { createLargeImportPlanFixture } from './importPlanFixture';

export function StressPlanFixture() {
  const { plan, client } = useMemo(() => {
    const stressPlan = createLargeImportPlanFixture();
    const progress: ReviewProgress = {
      import_run_id: stressPlan.import_run_id,
      total_review_candidates: 10_000,
      decided_count: 10_000,
      remaining_count: 0,
      all_decided: true,
    };
    const queryClient = new QueryClient({
      defaultOptions: { queries: { staleTime: Infinity, retry: false } },
    });
    queryClient.setQueryData(['reviewQueue', stressPlan.import_run_id], []);
    queryClient.setQueryData(['reviewProgress', stressPlan.import_run_id], progress);
    queryClient.setQueryData(
      ['reviewFrozenImportPlanSummary', stressPlan.import_run_id],
      stressPlan,
    );
    queryClient.setQueryData(['database-info-dashboard'], {
      imports: { failed_album_count: 0, pending_review_count: 0, recovery_required_run_count: 0 },
    });
    return { plan: stressPlan, client: queryClient };
  }, []);

  return (
    <QueryClientProvider client={client}>
      <Layout currentRoute="review" onNavigate={() => undefined} enablePolling={false}>
        <ReviewPage
          initialImportRunId={plan.import_run_id}
          initialPlan={plan}
          initialShowPlan
          enablePolling={false}
          onNavigate={() => undefined}
        />
      </Layout>
    </QueryClientProvider>
  );
}
