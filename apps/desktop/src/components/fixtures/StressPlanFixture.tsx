import { useMemo } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReviewProgress } from '../../lib/ipc/types';
import { PlanPage } from '../../pages/PlanPage';
import { Layout } from '../Layout';
import { createLargeImportPlanFixture } from './importPlanFixture';

export function StressPlanFixture() {
  const { plan, client } = useMemo(() => {
    const stressPlan = { ...createLargeImportPlanFixture(), plan_hash: null };
    const progress: ReviewProgress = {
      import_run_id: stressPlan.import_run_id,
      total_review_groups: 10_000,
      resolved_count: 10_000,
      remaining_count: 0,
      all_decided: true,
    };
    const queryClient = new QueryClient({
      defaultOptions: { queries: { staleTime: Infinity, retry: false } },
    });
    queryClient.setQueryData(['reviewQueue', stressPlan.import_run_id], []);
    queryClient.setQueryData(['reviewProgress', stressPlan.import_run_id], progress);
    queryClient.setQueryData(['importPlanDraftSummary', stressPlan.import_run_id], stressPlan);
    queryClient.setQueryData(['database-info-dashboard'], {
      imports: { failed_album_count: 0, pending_review_count: 0, recovery_required_run_count: 0 },
    });
    return { plan: stressPlan, client: queryClient };
  }, []);

  return (
    <QueryClientProvider client={client}>
      <Layout currentRoute="plan" onNavigate={() => undefined} enablePolling={false}>
        <PlanPage initialPlan={plan} enablePolling={false} onNavigate={() => undefined} />
      </Layout>
    </QueryClientProvider>
  );
}
