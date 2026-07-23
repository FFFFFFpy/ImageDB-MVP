import { useEffect, useState, type ReactNode } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { Layout } from '../components/Layout';
import { Button, PageHeader, Skeleton, StatusBanner } from '../components/ui';
import { useRouter, type Route } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import type { ImportWorkflowStage } from '../lib/ipc/types';
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

const WORKFLOW_ROUTES = new Set<Route>(['scan', 'review', 'plan', 'commit', 'recovery']);

export function routeForWorkflowStage(stage: ImportWorkflowStage): Route {
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
    case 'failed':
      return 'scan';
    case 'completed':
    case 'abandoned':
      return 'dashboard';
  }
}

export function App() {
  const { route, runId, fresh, navigate } = useRouter();
  const queryClient = useQueryClient();
  const [workflowNavigationBlocked, setWorkflowNavigationBlocked] = useState(false);

  const settings = useQuery({
    queryKey: ['settings'],
    queryFn: api.getSettings,
  });

  const needsOnboarding = settings.data ? !settings.data.first_run_completed : false;
  const showOnboarding = route === 'onboarding' || (needsOnboarding && route === 'dashboard');
  const needsWorkflowResolution =
    WORKFLOW_ROUTES.has(route) && !(route === 'scan' && fresh);
  const workflow = useQuery({
    queryKey: ['workflow-resolution', runId ?? 'current'],
    queryFn: () => api.resolveImportWorkflow(runId),
    enabled: needsWorkflowResolution && !showOnboarding,
    retry: false,
  });

  const resolvedRoute = workflow.data ? routeForWorkflowStage(workflow.data.stage) : route;
  const resolvedRunId = workflow.data?.import_run_id ?? runId;
  const workflowRouteMismatch =
    needsWorkflowResolution &&
    !!workflow.data &&
    (resolvedRoute !== route ||
      (WORKFLOW_ROUTES.has(resolvedRoute) && resolvedRunId !== runId));

  useEffect(() => {
    if (!workflowRouteMismatch || !workflow.data) return;
    navigate(resolvedRoute, {
      runId: WORKFLOW_ROUTES.has(resolvedRoute) ? resolvedRunId : null,
      replace: true,
    });
  }, [
    navigate,
    resolvedRoute,
    resolvedRunId,
    workflow.data,
    workflowRouteMismatch,
  ]);

  const handleLayoutNavigate = (nextRoute: Route) => {
    if (workflowNavigationBlocked) return;
    if (nextRoute === 'scan') {
      navigate('scan', { fresh: true });
      return;
    }
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

  let page: ReactNode = null;
  if (needsWorkflowResolution && (workflow.isLoading || workflowRouteMismatch)) {
    page = (
      <div className="workflow-resolution-page">
        <PageHeader title="正在恢复导入工作流" description="根据已持久化阶段定位当前任务。" />
        <Skeleton width="100%" height={320} />
      </div>
    );
  } else if (needsWorkflowResolution && workflow.isError) {
    page = (
      <div className="workflow-resolution-page">
        <PageHeader title="无法恢复导入工作流" />
        <StatusBanner
          tone="danger"
          title="工作流阶段查询失败"
          actions={<Button onClick={() => workflow.refetch()}>重新查询</Button>}
        >
          {String(workflow.error)}
        </StatusBanner>
      </div>
    );
  } else {
    const activeRoute = needsWorkflowResolution ? resolvedRoute : route;
    const activeRunId =
      activeRoute === 'scan' && fresh ? null : (workflow.data?.import_run_id ?? runId);

    page = (
      <>
        {activeRoute === 'dashboard' && (
          <DashboardPage
            needsOnboarding={needsOnboarding}
            onConfigureDatabase={() => navigate('settings')}
            onGoScan={(importRunId) =>
              importRunId
                ? navigate('scan', { runId: importRunId })
                : navigate('scan', { fresh: true })
            }
            onGoReview={(importRunId) => navigate('review', { runId: importRunId })}
            onGoCommit={(importRunId) => navigate('commit', { runId: importRunId })}
            onGoRecovery={() => navigate('recovery')}
            onGoLibrary={() => navigate('library')}
          />
        )}
        {activeRoute === 'library' && <LibraryPage onNavigate={navigate} />}
        {activeRoute === 'scan' && (
          <ScanPage
            initialImportRunId={activeRunId}
            onNavigate={navigate}
            onRunStarted={(importRunId) =>
              navigate('scan', { runId: importRunId, replace: true })
            }
            onGoReview={(importRunId) => navigate('review', { runId: importRunId })}
          />
        )}
        {activeRoute === 'review' && (
          <ReviewPage
            initialImportRunId={activeRunId}
            onNavigate={navigate}
            onGoPlan={(importRunId) => navigate('plan', { runId: importRunId })}
            onGoScan={(importRunId) => navigate('scan', { runId: importRunId })}
            onStartImport={() => navigate('scan', { fresh: true })}
          />
        )}
        {activeRoute === 'plan' && (
          <PlanPage
            initialImportRunId={activeRunId}
            onNavigate={navigate}
            onGoCommit={(importRunId) => navigate('commit', { runId: importRunId })}
            onNavigationBlockedChange={setWorkflowNavigationBlocked}
          />
        )}
        {activeRoute === 'commit' && (
          <CommitPage
            initialImportRunId={activeRunId}
            initialPhase={workflow.data?.stage === 'committing' ? 'committing' : 'confirm'}
            fileTransactionCount={workflow.data?.file_transaction_count ?? 0}
            onNavigate={navigate}
            onWorkflowAbandoned={handleWorkflowAbandoned}
            onNavigationBlockedChange={setWorkflowNavigationBlocked}
          />
        )}
        {activeRoute === 'recovery' && <RecoveryPage onNavigate={navigate} />}
        {activeRoute === 'settings' && (
          <SettingsPage onOpenProbes={() => navigate('probes')} />
        )}
        {activeRoute === 'probes' && <ProbesPage />}
      </>
    );
  }

  return (
    <ErrorBoundary>
      {showOnboarding ? (
        <OnboardingPage onComplete={handleOnboardingComplete} />
      ) : (
        <Layout
          currentRoute={needsWorkflowResolution ? resolvedRoute : route}
          onNavigate={handleLayoutNavigate}
          navigationDisabled={workflowNavigationBlocked}
        >
          {page}
        </Layout>
      )}
    </ErrorBoundary>
  );
}
