import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { Layout } from '../components/Layout';
import { useRouter } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import { CommitPage } from '../pages/CommitPage';
import { DashboardPage } from '../pages/DashboardPage';
import { LibraryPage } from '../pages/LibraryPage';
import { OnboardingPage } from '../pages/OnboardingPage';
import { PlanPage } from '../pages/PlanPage';
import { ProbesPage } from '../pages/ProbesPage';
import { RecoveryPage } from '../pages/RecoveryPage';
import { ReviewPage } from '../pages/ReviewPage';
import { ScanPage } from '../pages/ScanPage';
import { SettingsPage } from '../pages/SettingsPage';

export function App() {
  const { route, navigate } = useRouter();
  const queryClient = useQueryClient();
  const [workflowImportRunId, setWorkflowImportRunId] = useState<string | null>(null);
  const [workflowNavigationBlocked, setWorkflowNavigationBlocked] = useState(false);

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const needsOnboarding = settings.data ? !settings.data.first_run_completed : false;
  const showOnboarding = route === 'onboarding' || (needsOnboarding && route === 'dashboard');

  const handleLayoutNavigate = (nextRoute: Parameters<typeof navigate>[0]) => {
    if (
      nextRoute === 'scan' ||
      nextRoute === 'review' ||
      nextRoute === 'plan' ||
      nextRoute === 'commit'
    ) {
      setWorkflowImportRunId(null);
    }
    navigate(nextRoute);
  };

  const handleWorkflowAbandoned = () => {
    setWorkflowImportRunId(null);
    setWorkflowNavigationBlocked(false);
    navigate('dashboard');
  };

  const handleOnboardingComplete = async () => {
    const currentSettings = await api.getSettings();
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
        <Layout
          currentRoute={route}
          onNavigate={handleLayoutNavigate}
          navigationDisabled={workflowNavigationBlocked}
        >
          {route === 'dashboard' && (
            <DashboardPage
              needsOnboarding={needsOnboarding}
              onConfigureDatabase={() => navigate('settings')}
              onGoScan={(importRunId) => {
                setWorkflowImportRunId(importRunId ?? null);
                navigate('scan');
              }}
              onGoReview={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('review');
              }}
              onGoPlan={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('plan');
              }}
              onGoCommit={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('commit');
              }}
              onGoRecovery={() => navigate('recovery')}
              onGoLibrary={() => navigate('library')}
            />
          )}
          {route === 'library' && <LibraryPage onNavigate={navigate} />}
          {route === 'scan' && (
            <ScanPage
              initialImportRunId={workflowImportRunId}
              onNavigate={navigate}
              onGoReview={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('review');
              }}
            />
          )}
          {route === 'review' && (
            <ReviewPage
              initialImportRunId={workflowImportRunId}
              onNavigate={navigate}
              onGoPlan={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('plan');
              }}
            />
          )}
          {route === 'plan' && (
            <PlanPage
              initialImportRunId={workflowImportRunId}
              onNavigate={navigate}
              onGoCommit={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('commit');
              }}
              onWorkflowAbandoned={handleWorkflowAbandoned}
              onPlanEditPendingChange={setWorkflowNavigationBlocked}
            />
          )}
          {route === 'commit' && (
            <CommitPage
              initialImportRunId={workflowImportRunId}
              onNavigate={navigate}
              onGoReview={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('review');
              }}
              onGoPlan={(importRunId) => {
                setWorkflowImportRunId(importRunId);
                navigate('plan');
              }}
              onWorkflowAbandoned={handleWorkflowAbandoned}
              onNavigationBlockedChange={setWorkflowNavigationBlocked}
            />
          )}
          {route === 'recovery' && <RecoveryPage onNavigate={navigate} />}
          {route === 'settings' && <SettingsPage onOpenProbes={() => navigate('probes')} />}
          {route === 'probes' && <ProbesPage />}
        </Layout>
      )}
    </ErrorBoundary>
  );
}
