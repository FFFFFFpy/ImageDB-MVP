import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { DatabaseState } from '../../lib/ipc/types';
import { OnboardingPage } from '../../pages/OnboardingPage';

const databaseState: DatabaseState = {
  mode: null,
  status: 'not_initialized',
  managed_config: {
    data_dir: 'C:/Users/Helw/AppData/Local/ImageDB/postgres_data',
    port: 0,
    username: 'imagedb',
    database: 'imagedb',
  },
  external_config: null,
  pgvector_available: false,
  migration_version: null,
  diagnostics: [],
};

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

export function OnboardingFixture() {
  return (
    <QueryClientProvider client={fixtureClient}>
      <OnboardingPage
        initialDatabaseState={databaseState}
        enablePolling={false}
        onComplete={() => undefined}
      />
    </QueryClientProvider>
  );
}
