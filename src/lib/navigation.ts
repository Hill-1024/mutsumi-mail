import { useCallback } from 'react';
import { flushSync } from 'react-dom';
import { useNavigate, type NavigateOptions } from 'react-router-dom';

interface Transition {
  finished: Promise<void>;
  skipTransition: () => void;
}
let active: Transition | undefined;
let generation = 0;

/** Capture the navigation and its selection changes in the same commit. */
export function transitionView(update: () => void): void {
  const current = ++generation;
  active?.skipTransition();
  const apply = () => {
    if (generation === current) flushSync(update);
  };
  const doc = document as Document & {
    startViewTransition?: (callback: () => void) => Transition;
  };
  if (!doc.startViewTransition || window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    delete document.documentElement.dataset.viewTransition;
    active = undefined;
    apply();
    return;
  }
  document.documentElement.dataset.viewTransition = 'active';
  try {
    active = doc.startViewTransition(apply);
    void active.finished.catch(() => undefined).finally(() => {
      if (current !== generation) return;
      delete document.documentElement.dataset.viewTransition;
      active = undefined;
    });
  } catch {
    delete document.documentElement.dataset.viewTransition;
    active = undefined;
    apply();
  }
}

export function useAppNavigation() {
  const navigate = useNavigate();
  return useCallback((path: string, update?: () => void, options?: NavigateOptions) => {
    transitionView(() => {
      update?.();
      void navigate(path, options);
    });
  }, [navigate]);
}
