import { useEffect, useMemo, useRef, useState } from 'react';
import { BrowserRouter, Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import { keepPreviousData, QueryClient, QueryClientProvider, useQuery, useQueryClient } from '@tanstack/react-query';
import { listen } from '@tauri-apps/api/event';
import { AccountWizard } from './components/AccountWizard';
import { AppShell } from './components/AppShell';
import { ComposeDialog } from './components/ComposeDialog';
import { MailHome } from './components/MailHome';
import { OutboxView, SearchView, SettingsView } from './components/UtilityViews';
import { appErrorMessage, isTauriRuntime, getMessage, listAccounts, listMailboxes, listMessages, listOutbox, removeAccount, startSync, syncAll } from './lib/tauri';
import { useExitPresence } from './lib/motion';
import { readMailData } from './lib/optimistic-flags';
import { subscribeToMailCache } from './lib/mail-cache-events';
import type { Account, OutboxItem } from './types';
import { useUiStore } from './stores/ui';

const queryClient = new QueryClient({ defaultOptions: { queries: { staleTime: 30_000, retry: 1 } } });

const canSyncAccount = (account: Pick<Account, 'enabled' | 'incomingConfigured'>) => (
  account.enabled && account.incomingConfigured
);

function MailApp() {
  const navigate = useNavigate();
  const [accountWizardOpen, setAccountWizardOpen] = useState(false);
  const [selectedAccountId, setSelectedAccountId] = useState<string | null>(null);
  const initialRouteNormalized = useRef(false);
  const queryClient = useQueryClient();
  const { selectedMailboxId, selectedMessageId, selectMailbox, composeOpen, setComposeOpen, setSyncMessage } = useUiStore();
  const accounts = useQuery({ queryKey: ['accounts'], queryFn: listAccounts });
  const accountItems = useMemo(() => accounts.data ?? [], [accounts.data]);
  const scopedAccountId = selectedAccountId && accountItems.some((account) => account.id === selectedAccountId)
    ? selectedAccountId
    : null;
  const mailboxes = useQuery({
    queryKey: ['mailboxes', scopedAccountId ?? 'all'],
    queryFn: () => readMailData(queryClient, () => listMailboxes(scopedAccountId ?? undefined)),
    enabled: accountItems.length > 0,
  });
  const mailboxItems = useMemo(() => mailboxes.data ?? [], [mailboxes.data]);
  const selectedMailboxExists = mailboxItems.some((mailbox) => mailbox.id === selectedMailboxId);
  const isVirtualMailbox = selectedMailboxId === 'inbox' || selectedMailboxId === 'starred';
  const messages = useQuery({
    queryKey: ['messages', scopedAccountId ?? 'all', selectedMailboxId],
    enabled: accountItems.length > 0 && (isVirtualMailbox || selectedMailboxExists),
    // Show the previous folder's list (dimmed by MessageList) while the next folder
    // loads instead of flashing an empty/spinner state on every switch.
    placeholderData: (previous, query) => query?.queryKey[1] === (scopedAccountId ?? 'all')
      ? keepPreviousData(previous) : undefined,
    queryFn: () => readMailData(queryClient, async () => {
      if (selectedMailboxId === 'inbox') {
        return listMessages({ accountId: scopedAccountId ?? undefined, mailboxRole: 'inbox', limit: 200 });
      }
      if (selectedMailboxId === 'starred') {
        return listMessages({ accountId: scopedAccountId ?? undefined, isStarred: true, limit: 200 });
      }
      return listMessages({ accountId: scopedAccountId ?? undefined, mailboxId: selectedMailboxId, limit: 200 });
    }),
  });
  const outbox = useQuery({
    queryKey: ['outbox', scopedAccountId ?? 'all'],
    queryFn: () => listOutbox(scopedAccountId ?? undefined),
    enabled: accountItems.length > 0,
  });

  // Search may select an older message outside the bounded folder list. Resolve
  // its exact mailbox instance instead of clearing selection and opening row one.
  const selectedInList = messages.data?.some((message) => message.id === selectedMessageId) ?? false;
  const selectedDetailEnabled = Boolean(selectedMessageId) && !isVirtualMailbox && selectedMailboxExists
    && !messages.isPending && !messages.isPlaceholderData && !selectedInList;
  const selectedDetail = useQuery({
    queryKey: ['message', selectedMailboxId, selectedMessageId],
    queryFn: () => {
      if (!selectedMessageId) throw new Error('尚未选择邮件');
      return readMailData(queryClient, () => getMessage(selectedMessageId, selectedMailboxId));
    },
    enabled: selectedDetailEnabled,
  });
  const currentMessages = useMemo(() => {
    const items = messages.data ?? [];
    const detail = selectedDetailEnabled ? selectedDetail.data : undefined;
    return detail && detail.id === selectedMessageId && detail.mailboxId === selectedMailboxId
      ? [...items, detail] : items;
  }, [messages.data, selectedDetail.data, selectedDetailEnabled, selectedMailboxId, selectedMessageId]);
  const refreshSync = () => {
    if (accountItems.length === 0) return;
    const scopedAccount = scopedAccountId
      ? accountItems.find((account) => account.id === scopedAccountId)
      : undefined;
    if (scopedAccountId && (!scopedAccount || !canSyncAccount(scopedAccount))) {
      setSyncMessage('当前账户仅支持发件，无法同步邮件');
      return;
    }
    if (!scopedAccountId && !accountItems.some(canSyncAccount)) {
      setSyncMessage('当前没有可同步的收件账户');
      return;
    }
    setSyncMessage(scopedAccountId ? '正在同步…' : '正在同步所有账户…');
    const task = scopedAccountId ? startSync(scopedAccountId).then(() => undefined) : syncAll().then(() => undefined);
    void task.catch((error) => setSyncMessage(appErrorMessage(error)));
  };

  useEffect(() => {
    if (accounts.isLoading || initialRouteNormalized.current) return;
    initialRouteNormalized.current = true;
    if (accountItems.length === 0) navigate('/mail', { replace: true });
  }, [accountItems.length, accounts.isLoading, navigate]);

  useEffect(() => {
    if (mailboxes.isPending || mailboxes.isError || accountItems.length === 0 || isVirtualMailbox || selectedMailboxExists) return;
    selectMailbox('inbox');
  }, [accountItems.length, isVirtualMailbox, mailboxes.isPending, mailboxes.isError, selectMailbox, selectedMailboxExists]);

  useEffect(() => {
    if (!isTauriRuntime) return undefined;
    return subscribeToMailCache(queryClient);
  }, [queryClient]);

  useEffect(() => {
    if (!isTauriRuntime) return undefined;
    let cancelled = false;
    let unlistenSync: (() => void) | undefined;
    let unlistenOutbox: (() => void) | undefined;
    void listen<{ accountId: string; message?: string; state?: string }>('sync-progress', (event) => {
      if (scopedAccountId === event.payload.accountId) {
        setSyncMessage(event.payload.state === 'idle' ? '已同步 · 刚刚' : event.payload.message ?? '正在同步…');
      } else if (scopedAccountId === null) {
        // A unified inbox can have several concurrent states; let AppShell derive
        // the aggregate label from every account instead of showing whichever
        // event happened to arrive last.
        setSyncMessage(null);
      }
      void queryClient.invalidateQueries({ queryKey: ['accounts'] });
      if (event.payload.state === 'idle' || event.payload.state === 'partial' || event.payload.state === 'error') {
        void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
        void queryClient.invalidateQueries({ queryKey: ['messages'] });
        void queryClient.invalidateQueries({ queryKey: ['message'] });
        void queryClient.invalidateQueries({ queryKey: ['search'] });
      }
    }).then((dispose) => {
      // Cleanup can run before the async listen() resolves (fast account switching);
      // dispose immediately so the stale-scope listener never sticks around.
      if (cancelled) dispose();
      else unlistenSync = dispose;
    }).catch((error) => { if (!cancelled) setSyncMessage(appErrorMessage(error)); });
    void listen('outbox-changed', () => { void queryClient.invalidateQueries({ queryKey: ['outbox'] }); }).then((dispose) => {
      if (cancelled) dispose();
      else unlistenOutbox = dispose;
    }).catch((error) => { if (!cancelled) setSyncMessage(appErrorMessage(error)); });
    return () => {
      cancelled = true;
      unlistenSync?.();
      unlistenOutbox?.();
    };
  }, [queryClient, scopedAccountId, setSyncMessage]);

  const handleAccountSaved = (account: Account) => {
    const shouldStartSync = canSyncAccount(account);
    queryClient.setQueryData<Account[]>(['accounts'], (current = []) => [
      account,
      ...current.filter((item) => item.id !== account.id),
    ]);
    setSelectedAccountId(account.id);
    selectMailbox('inbox');
    setAccountWizardOpen(false);
    navigate(shouldStartSync ? '/mail' : '/outbox');
    void queryClient.invalidateQueries({ queryKey: ['accounts'] });
    void queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
    void queryClient.invalidateQueries({ queryKey: ['outbox'] });
  };

  const handleRemoveAccount = async (accountId: string) => {
    await removeAccount(accountId);
    if (selectedAccountId === accountId || scopedAccountId === null) {
      setSelectedAccountId(null);
      selectMailbox('inbox');
    }
    queryClient.removeQueries({ queryKey: ['search'] });
    queryClient.removeQueries({ queryKey: ['messages'] });
    queryClient.removeQueries({ queryKey: ['message'] });
    await queryClient.invalidateQueries({ queryKey: ['accounts'] });
    await queryClient.invalidateQueries({ queryKey: ['mailboxes'] });
    await queryClient.invalidateQueries({ queryKey: ['messages'] });
    await queryClient.invalidateQueries({ queryKey: ['outbox'] });
  };

  // Keep dialogs mounted while their exit animations play (MD3 emphasized easing).
  const composePresence = useExitPresence(composeOpen, 180);
  const wizardPresence = useExitPresence(accountWizardOpen, 180);

  if (accounts.isLoading) {
    return <main className="startup-state" role="status"><span className="spinner" /><span>正在打开本地邮箱…</span></main>;
  }

  if (accounts.isError) {
    return (
      <main className="startup-state" role="alert">
        <p>{appErrorMessage(accounts.error)}</p>
        <button className="primary-action" type="button" onClick={() => void accounts.refetch()}>重试</button>
      </main>
    );
  }

  return (
    <>
      <AppShell
        accounts={accountItems}
        selectedAccountId={scopedAccountId}
        onSelectAccount={(accountId) => {
          const account = accountItems.find((item) => item.id === accountId);
          setSelectedAccountId(accountId);
          setSyncMessage(null);
          selectMailbox('inbox');
          navigate(!account || account.incomingConfigured ? '/mail' : '/outbox');
        }}
        mailboxes={mailboxItems}
        messageCount={mailboxItems.filter((mailbox) => mailbox.specialRole === 'inbox').reduce((total, mailbox) => total + mailbox.unreadCount, 0)}
        onAddAccount={() => setAccountWizardOpen(true)}
      >
        <Routes>
          <Route path="/mail" element={<MailHome accounts={accountItems} hasAccounts={accountItems.length > 0} messages={currentMessages} mailboxes={mailboxItems} isLoading={!mailboxes.isError && (messages.isPending || (selectedDetailEnabled && selectedDetail.isPending))} isRefreshing={messages.isPlaceholderData} loadError={mailboxes.error || messages.error ? appErrorMessage(mailboxes.error ?? messages.error) : undefined} onRetry={() => { void mailboxes.refetch(); void messages.refetch(); }} onSync={refreshSync} onOpenSettings={() => navigate('/settings')} />} />
          <Route path="/search" element={<SearchView accountId={scopedAccountId ?? undefined} accounts={accountItems} messages={currentMessages} />} />
          <Route path="/outbox" element={<OutboxView accounts={accountItems} items={outbox.data ?? []} />} />
          <Route path="/settings/*" element={<SettingsView accounts={accountItems} onAddAccount={() => setAccountWizardOpen(true)} onRemoveAccount={handleRemoveAccount} />} />
          <Route path="*" element={<Navigate to="/mail" replace />} />
        </Routes>
      </AppShell>
      {composePresence.mounted && accountItems.some((account) => account.enabled && account.outgoingConfigured) && <ComposeDialog accounts={accountItems} defaultAccountId={scopedAccountId ?? undefined} closing={composePresence.exiting} onAnimationEnd={composePresence.handleAnimationEnd} onQueued={(item) => {
        queryClient.setQueryData<OutboxItem[]>(['outbox', 'all'], (current = []) => [item, ...current.filter((existing) => existing.id !== item.id)]);
      }} onClose={() => setComposeOpen(false)} />}
      {wizardPresence.mounted && <AccountWizard canClose closing={wizardPresence.exiting} onAnimationEnd={wizardPresence.handleAnimationEnd} onClose={() => setAccountWizardOpen(false)} onSaved={handleAccountSaved} />}
    </>
  );
}

export default function App({ client = queryClient }: { client?: QueryClient } = {}) {
  return (
    <QueryClientProvider client={client}>
      <BrowserRouter>
        <MailApp />
      </BrowserRouter>
    </QueryClientProvider>
  );
}
