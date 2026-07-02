import { useQuery } from '@tanstack/react-query';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { Layout } from '../components/Layout';
import { useRouter } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import { DashboardPage } from '../pages/DashboardPage';
import { OnboardingPage } from '../pages/OnboardingPage';
import { ProbesPage } from '../pages/ProbesPage';
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
          {route === 'scan' && <ScanPage />}
          {route === 'settings' && <SettingsPage />}
          {route === 'probes' && <ProbesPage />}
        </Layout>
      )}
    </ErrorBoundary>
  );
}
