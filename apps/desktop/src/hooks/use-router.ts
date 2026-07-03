import { useCallback, useEffect, useState } from 'react';

export type Route = 'onboarding' | 'dashboard' | 'settings' | 'probes' | 'scan' | 'review' | 'commit';

function getRouteFromHash(): Route {
  const hash = window.location.hash.slice(1) || '/';
  switch (hash) {
    case '/onboarding':
      return 'onboarding';
    case '/settings':
      return 'settings';
    case '/probes':
      return 'probes';
    case '/scan':
      return 'scan';
    case '/review':
      return 'review';
    case '/commit':
      return 'commit';
    default:
      return 'dashboard';
  }
}

export function useRouter() {
  const [route, setRoute] = useState<Route>(getRouteFromHash);

  useEffect(() => {
    const handler = () => setRoute(getRouteFromHash());
    window.addEventListener('hashchange', handler);
    return () => window.removeEventListener('hashchange', handler);
  }, []);

  const navigate = useCallback((to: Route) => {
    window.location.hash = `/${to}`;
  }, []);

  return { route, navigate };
}
