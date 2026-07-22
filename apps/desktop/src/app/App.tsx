import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { Layout } from '../components/Layout';
import { useRouter } from '../hooks/use-router';
import type { Route } from '../hooks/use-router';
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

const WORKFLOW_ROUTES: Route[] = ['review', 'plan', 'commit'];

function stageToRoute(stage: string): Route {
  switch (stage) {
    case 'analysis':
      return 'scan';
    case 'review':
    case 'generate_plan':
      return 'review';
    case 'plan_draft':
      return 'plan';
    case 'commit_confirm':
    case 'committing':
      return 'commit';
    case 'recovery':
      return 'recovery';
    default:
      return 'dashboard';
  }
}

export function App() {
  const { route, runId, navigate } = useRouter();
  const queryClient = useQueryClient();
  const [workflowNavigationBlocked, setWorkflowNavigationBlocked] = useState(false);
  const resolvingRef = useRef(false);

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const needsOnboarding = settings.data ? !settings.data.first_run_completed : false;
  const showOnboarding = route === 'onboarding' || (needsOnboarding && route === 'dashboard');

  useEffect(() => {
    if (!WORKFLOW_ROUTES.includes(route) || runId || resolvingRef.current) return;
    resolvingRef.current = true;
    api
      .getImportWorkflowStage()
      .then((stage) => {
        if (stage.import_run_id) {
          const target = stageToRoute(stage.stage);
          navigate(target, stage.import_run_id);
        } else {
          navigate('dashboard');
        }
      })
      .catch(() => navigate('dashboard'))
      .finally(() => {
        resolvingRef.current = false;
      });
  }, [route, runId, navigate]);

  const handleLayoutNavigate = (nextRoute: Route) => {
    navigate(nextRoute);
  };

  const handleWorkflowAbandoned = () => {
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
              onGoScan={(importRunId) => navigate('scan', importRunId)}
              onGoReview={(importRunId) => navigate('review', importRunId)}
              onGoCommit={(importRunId) => navigate('commit', importRunId)}
              onGoRecovery={() => navigate('recovery')}
              onGoLibrary={() => navigate('library')}
            />
          )}
          {route === 'library' && <LibraryPage onNavigate={navigate} />}
          {route === 'scan' && (
            <ScanPage
              key={runId ?? 'none'}
              initialImportRunId={runId}
              onNavigate={navigate}
              onGoReview={(importRunId) => navigate('review', importRunId)}
            />
          )}
          {route === 'review' && (
            <ReviewPage
              key={runId ?? 'none'}
              initialImportRunId={runId}
              onNavigate={navigate}
              onGoPlan={(importRunId) => navigate('plan', importRunId)}
              onWorkflowAbandoned={handleWorkflowAbandoned}
            />
          )}
          {route === 'plan' && (
            <PlanPage
              key={runId ?? 'none'}
              initialImportRunId={runId}
              onNavigate={navigate}
              onGoCommit={(importRunId) => navigate('commit', importRunId)}
              onWorkflowAbandoned={handleWorkflowAbandoned}
              onNavigationBlockedChange={setWorkflowNavigationBlocked}
            />
          )}
          {route === 'commit' && (
            <CommitPage
              key={runId ?? 'none'}
              initialImportRunId={runId}
              onNavigate={navigate}
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
