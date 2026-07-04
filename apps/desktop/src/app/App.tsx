import { useQuery } from '@tanstack/react-query';
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

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const needsOnboarding = settings.data ? !settings.data.first_run_completed : false;
  const showOnboarding = route === 'onboarding' || (needsOnboarding && route === 'dashboard');

  const handleOnboardingComplete = () => {
    navigate('dashboard');
    window.location.reload();
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
