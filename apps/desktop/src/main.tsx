import React from 'react';
import ReactDOM from 'react-dom/client';
import { QueryClientProvider } from '@tanstack/react-query';
import { App } from './app/App';
import { queryClient } from './app/query-client';
import { UiShowcase } from './components/ui';
import 'animal-island-ui/style';
import './styles/tokens.css';
import './styles/global.css';
import './styles/ui.css';

const showM3FoundationFixture =
  import.meta.env.DEV &&
  new URLSearchParams(window.location.search).get('m3-fixture') === 'foundation';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {showM3FoundationFixture ? (
      <UiShowcase />
    ) : (
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    )}
  </React.StrictMode>,
);
