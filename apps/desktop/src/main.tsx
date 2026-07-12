import React from 'react';
import ReactDOM from 'react-dom/client';
import { QueryClientProvider } from '@tanstack/react-query';
import { App } from './app/App';
import { queryClient } from './app/query-client';
import { DashboardFixture } from './components/fixtures/DashboardFixture';
import { ScanFixture } from './components/fixtures/ScanFixture';
import { ReviewFixture } from './components/fixtures/ReviewFixture';
import { CommitFixture, type CommitFixtureState } from './components/fixtures/CommitFixture';
import { UiShowcase } from './components/ui';
import 'animal-island-ui/style';
import './styles/tokens.css';
import './styles/global.css';
import './styles/ui.css';
import './styles/layout.css';
import './styles/dashboard.css';
import './styles/scan.css';
import './styles/review.css';
import './styles/plan.css';
import './styles/commit.css';

const showM3FoundationFixture =
  import.meta.env.DEV &&
  new URLSearchParams(window.location.search).get('m3-fixture') === 'foundation';
const showM3DashboardFixture =
  import.meta.env.DEV &&
  new URLSearchParams(window.location.search).get('m3-fixture') === 'dashboard';
const showM3ScanFixture =
  import.meta.env.DEV && new URLSearchParams(window.location.search).get('m3-fixture') === 'scan';
const showM3ReviewFixture =
  import.meta.env.DEV && new URLSearchParams(window.location.search).get('m3-fixture') === 'review';
const showM3PlanFixture =
  import.meta.env.DEV && new URLSearchParams(window.location.search).get('m3-fixture') === 'plan';
const showM3CommitFixture =
  import.meta.env.DEV && new URLSearchParams(window.location.search).get('m3-fixture') === 'commit';
const commitFixtureState =
  (new URLSearchParams(window.location.search).get('m3-state') as CommitFixtureState | null) ??
  'confirm';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {showM3FoundationFixture ? (
      <UiShowcase />
    ) : showM3DashboardFixture ? (
      <DashboardFixture />
    ) : showM3ScanFixture ? (
      <ScanFixture />
    ) : showM3ReviewFixture ? (
      <ReviewFixture />
    ) : showM3PlanFixture ? (
      <ReviewFixture view="plan" />
    ) : showM3CommitFixture ? (
      <CommitFixture state={commitFixtureState} />
    ) : (
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    )}
  </React.StrictMode>,
);
