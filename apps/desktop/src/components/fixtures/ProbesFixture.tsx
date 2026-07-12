import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { ProbesPage } from '../../pages/ProbesPage';
import { Layout } from '../Layout';

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['database-info-dashboard'], {
  imports: { failed_album_count: 0, pending_review_count: 0, recovery_required_run_count: 0 },
});

export function ProbesFixture() {
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout currentRoute="probes" onNavigate={() => undefined} enablePolling={false}>
        <ProbesPage />
      </Layout>
    </QueryClientProvider>
  );
}
