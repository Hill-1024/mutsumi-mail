import { useEffect, useMemo, useState } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { listen } from '@tauri-apps/api/event';
import { AccountWizard } from './components/AccountWizard';
import { AppShell } from './components/AppShell';
import { ComposeDialog } from './components/ComposeDialog';
import { MailHome } from './components/MailHome';
import { OutboxView, SearchView, SettingsView } from './components/UtilityViews';
import { isTauriRuntime, listAccounts, listMailboxes, listMessages, listOutbox, startSync } from './lib/tauri';
import type { OutboxItem } from './types';
import { useUiStore } from './stores/ui';

const queryClient = new QueryClient({ defaultOptions: { queries: { staleTime: 30_000, retry: 1 } } });

function MailApp() {
  const [accountWizardOpen, setAccountWizardOpen] = useState(false);
  const [optimisticOutbox, setOptimisticOutbox] = useState<OutboxItem[]>([]);
  const { selectedMailboxId, searchOpen, composeOpen, setComposeOpen, setSyncMessage } = useUiStore();
  const accounts = useQuery({ queryKey: ['accounts'], queryFn: listAccounts });
  const activeAccountId = accounts.data?.[0]?.id;
  const mailboxes = useQuery({ queryKey: ['mailboxes', activeAccountId], queryFn: () => listMailboxes(activeAccountId), enabled: Boolean(activeAccountId) });
  const messages = useQuery({ queryKey: ['messages', selectedMailboxId, searchOpen], queryFn: () => listMessages({ mailboxId: selectedMailboxId, limit: 200 }) });
  const outbox = useQuery({ queryKey: ['outbox', activeAccountId], queryFn: () => listOutbox(activeAccountId), enabled: Boolean(activeAccountId) });

  const currentMessages = useMemo(() => messages.data ?? [], [messages.data]);
  const refreshSync = () => {
    if (!activeAccountId) return;
    setSyncMessage('同步任务已排队…');
    void startSync(activeAccountId);
  };

  useEffect(() => {
    if (!isTauriRuntime) return undefined;
    let unlistenSync: (() => void) | undefined;
    let unlistenOutbox: (() => void) | undefined;
    void listen<{ message?: string; state?: string }>('sync-progress', (event) => {
      setSyncMessage(event.payload.state === 'idle' ? '已同步 · 刚刚' : event.payload.message ?? '正在同步…');
    }).then((dispose) => { unlistenSync = dispose; });
    void listen('outbox-changed', () => { void queryClient.invalidateQueries({ queryKey: ['outbox'] }); }).then((dispose) => { unlistenOutbox = dispose; });
    return () => { unlistenSync?.(); unlistenOutbox?.(); };
  }, [setSyncMessage]);

  return (
    <>
      <AppShell
        account={accounts.data?.[0]}
        mailboxes={mailboxes.data ?? []}
        messageCount={currentMessages.length}
        onAddAccount={() => setAccountWizardOpen(true)}
      >
        <Routes>
          <Route path="/mail" element={<MailHome messages={currentMessages} mailboxes={mailboxes.data ?? []} isLoading={messages.isLoading} onSync={refreshSync} />} />
          <Route path="/search" element={<SearchView messages={currentMessages} />} />
          <Route path="/outbox" element={<OutboxView items={optimisticOutbox.length ? optimisticOutbox : (outbox.data ?? [])} />} />
          <Route path="/settings/*" element={<SettingsView />} />
          <Route path="*" element={<Navigate to="/mail" replace />} />
        </Routes>
      </AppShell>
      {composeOpen && <ComposeDialog accountId={activeAccountId ?? 'demo-account'} onQueued={(item) => setOptimisticOutbox((current) => [item, ...current.filter((existing) => existing.id !== item.id)])} onClose={() => setComposeOpen(false)} />}
      {accountWizardOpen && <AccountWizard onClose={() => setAccountWizardOpen(false)} onSaved={() => { void queryClient.invalidateQueries({ queryKey: ['accounts'] }); setAccountWizardOpen(false); }} />}
    </>
  );
}

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <MailApp />
      </BrowserRouter>
    </QueryClientProvider>
  );
}
