import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { Layout } from '../components/Layout';
import { useRouter } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import { CommitPage } from '../pages/CommitPage';
import { DashboardPage } from '../pages/DashboardPage';
import { OnboardingPage } from '../pages/OnboardingPage';
import { ProbesPage } from '../pages/ProbesPage';
import { RecoveryPage } from '../pages/RecoveryPage';
import { ReviewPage } from '../pages/ReviewPage';
import { ScanPage } from '../pages/ScanPage';
import { SettingsPage } from '../pages/SettingsPage';

export function App() {
  const { route, navigate } = useRouter();
  const queryClient = useQueryClient();

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const needsOnboarding = settings.data ? !settings.data.first_run_completed : false;
  const showOnboarding = route === 'onboarding' || (needsOnboarding && route === 'dashboard');

  const handleOnboardingComplete = async () => {
    const currentSettings = settings.data ?? (await api.getSettings());
    if (!currentSettings.first_run_completed) {
      await api.updateSettings({
        ...currentSettings,
        first_run_completed: true,
      });
    }

    // Invalidate settings + database status so first_run_completed and the
    // connected DB state are re-fetched fresh, then navigate without a full
    // page reload (a reload would drop all in-memory React state and force
    // the user to re-wait while the DB status poll repopulates).
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ['settings'] }),
      queryClient.invalidateQueries({ queryKey: ['database-status'] }),
    ]);
    await queryClient.refetchQueries({ queryKey: ['settings'], type: 'active' });
    navigate('dashboard');
  };

  return (
    <ErrorBoundary>
      {showOnboarding ? (
        <OnboardingPage onComplete={handleOnboardingComplete} />
      ) : (
        <Layout currentRoute={route} onNavigate={navigate}>
          {route === 'dashboard' && (
            <DashboardPage
              needsOnboarding={needsOnboarding}
              onGoOnboarding={() => navigate('onboarding')}
              onGoScan={() => navigate('scan')}
            />
          )}
          {route === 'scan' && <ScanPage onNavigate={navigate} />}
          {route === 'review' && <ReviewPage onNavigate={navigate} />}
          {route === 'commit' && <CommitPage onNavigate={navigate} />}
          {route === 'recovery' && <RecoveryPage onNavigate={navigate} />}
          {route === 'settings' && <SettingsPage />}
          {route === 'probes' && <ProbesPage />}
        </Layout>
      )}
    </ErrorBoundary>
  );
}
