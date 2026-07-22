import { useCallback, useEffect, useState } from 'react';

export type Route =
  | 'onboarding'
  | 'dashboard'
  | 'library'
  | 'settings'
  | 'probes'
  | 'scan'
  | 'review'
  | 'plan'
  | 'commit'
  | 'recovery';

export interface RouteState {
  route: Route;
  runId: string | null;
}

function parseHash(): RouteState {
  const raw = window.location.hash.slice(1) || '/';
  const [path, query] = raw.split('?');
  const params = new URLSearchParams(query ?? '');
  const runId = params.get('run');

  const route = ((): Route => {
    switch (path) {
      case '/onboarding':
        return 'onboarding';
      case '/settings':
        return 'settings';
      case '/library':
        return 'library';
      case '/probes':
        return 'probes';
      case '/scan':
        return 'scan';
      case '/review':
        return 'review';
      case '/plan':
        return 'plan';
      case '/commit':
        return 'commit';
      case '/recovery':
        return 'recovery';
      default:
        return 'dashboard';
    }
  })();

  return { route, runId };
}

export function useRouter() {
  const [state, setState] = useState<RouteState>(parseHash);

  useEffect(() => {
    const handler = () => setState(parseHash());
    window.addEventListener('hashchange', handler);
    return () => window.removeEventListener('hashchange', handler);
  }, []);

  const navigate = useCallback((to: Route, runId?: string | null) => {
    if (runId) {
      window.location.hash = `/${to}?run=${runId}`;
    } else {
      window.location.hash = `/${to}`;
    }
  }, []);

  return { route: state.route, runId: state.runId, navigate };
}
