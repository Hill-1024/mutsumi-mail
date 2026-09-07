import type { QueryClient } from '@tanstack/react-query';
import { listen } from '@tauri-apps/api/event';

/** Subscribe once per query client, including across account/folder changes. */
export function subscribeToMailCache(queryClient: QueryClient): () => void {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  const refresh = () => {
    refreshTimer = undefined;
    for (const key of ['mailboxes', 'messages', 'message', 'search']) {
      void queryClient.invalidateQueries({ queryKey: [key] });
    }
  };
  const scheduleRefresh = () => {
    if (disposed || refreshTimer !== undefined) return;
    // Coalesce bursts without postponing display indefinitely during a long sync.
    refreshTimer = setTimeout(refresh, 100);
  };
  void listen('mail-cache-changed', scheduleRefresh).then((dispose) => {
    if (disposed) dispose();
    else {
      unlisten = dispose;
      // A startup sync may have committed between the first query and registration.
      scheduleRefresh();
    }
  }).catch((error: unknown) => {
    // Terminal sync-progress events still refresh the cache if this subscription fails.
    console.error('Unable to subscribe to mail cache changes', error);
  });
  return () => {
    disposed = true;
    if (refreshTimer !== undefined) clearTimeout(refreshTimer);
    unlisten?.();
  };
}
