// @vitest-environment jsdom

import { act } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import type { Account, Message, ProviderPreset } from '../types';
import { useUiStore } from '../stores/ui';

const apiMocks = vi.hoisted(() => ({
  cancelOutboxItem: vi.fn(),
  clearCache: vi.fn(),
  createAccount: vi.fn(),
  deleteMessages: vi.fn(),
  fetchMessageBody: vi.fn(),
  getSettings: vi.fn(),
  listAccounts: vi.fn(),
  listMailboxes: vi.fn(),
  listMessages: vi.fn(),
  listOutbox: vi.fn(),
  moveMessages: vi.fn(),
  mutateMessage: vi.fn(),
  mutateMessages: vi.fn(),
  removeAccount: vi.fn(),
  retryOutboxItem: vi.fn(),
  saveDraft: vi.fn(),
  searchMessages: vi.fn(),
  sendDraft: vi.fn(),
  sendDraftWithAttachments: vi.fn(),
  startSync: vi.fn(),
  syncAll: vi.fn(),
  updateSettings: vi.fn(),
}));

const providerPresets = vi.hoisted(
  () =>
    [
      {
        id: 'qq',
        displayName: 'QQ 邮箱',
        emailDomainPatterns: ['qq.com'],
        incoming: {
          protocol: 'imap',
          host: 'imap.qq.com',
          port: 993,
          tlsMode: 'implicit',
          authMethods: ['password'],
        },
        outgoing: {
          protocol: 'smtp',
          host: 'smtp.qq.com',
          port: 465,
          tlsMode: 'implicit',
          authMethods: ['password'],
        },
        helpText: '使用客户端授权码，不是 QQ 登录密码。',
        capabilities: {},
        quirks: [],
      },
      {
        id: 'netease-163',
        displayName: '网易 163 邮箱',
        emailDomainPatterns: ['163.com'],
        incoming: {
          protocol: 'imap',
          host: 'imap.163.com',
          port: 993,
          tlsMode: 'implicit',
          authMethods: ['password'],
        },
        outgoing: {
          protocol: 'smtp',
          host: 'smtp.163.com',
          port: 465,
          tlsMode: 'implicit',
          authMethods: ['password'],
        },
        helpText: '使用客户端授权码，不是网页登录密码。',
        capabilities: {},
        quirks: [],
      },
      {
        id: 'generic',
        displayName: '通用 IMAP + SMTP',
        emailDomainPatterns: [],
        incoming: {
          protocol: 'imap',
          host: '',
          port: 993,
          tlsMode: 'implicit',
          authMethods: ['password'],
        },
        outgoing: {
          protocol: 'smtp',
          host: '',
          port: 465,
          tlsMode: 'implicit',
          authMethods: ['password'],
        },
        helpText: '手动填写 IMAP 与 SMTP 端点。',
        capabilities: {},
        quirks: [],
      },
      {
        id: 'generic-smtp',
        displayName: '通用 SMTP（仅发件）',
        emailDomainPatterns: [],
        outgoing: {
          protocol: 'smtp',
          host: '',
          port: 465,
          tlsMode: 'implicit',
          authMethods: ['password'],
        },
        helpText: '手动填写 SMTP 发件端点。此账户只用于发件。',
        capabilities: {},
        quirks: ['仅发件'],
      },
    ] satisfies ProviderPreset[],
);

vi.mock('../lib/tauri', () => ({
  ...apiMocks,
  isTauriRuntime: false,
  providerPresets,
  appErrorMessage: (error: unknown) => {
    if (typeof error === 'object' && error && 'message' in error) {
      return String(error.message);
    }
    return error instanceof Error ? error.message : '发生未知错误，请稍后重试';
  },
}));

vi.mock('../lib/icons', () => ({
  Icon: ({ name }: { name: string }) => <span data-testid={`icon-${name}`} aria-hidden="true" />,
}));

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getTotalSize: () => count * 92,
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, start: index * 92 })),
  }),
}));

import App from '../App';
import { AccountWizard } from './AccountWizard';
import { ComposeDialog } from './ComposeDialog';
import { MailHome } from './MailHome';
import { MessageList } from './MessageList';
import { Reader } from './Reader';

const account = (overrides: Partial<Account> = {}): Account => ({
  id: 'account-a',
  providerId: 'qq',
  email: 'first@qq.com',
  displayName: '第一个账户',
  enabled: true,
  syncPolicy: 'automatic',
  incomingConfigured: true,
  outgoingConfigured: true,
  syncStatus: 'idle',
  ...overrides,
});

const message = (overrides: Partial<Message> = {}): Message => ({
  id: 'message-1',
  accountId: 'account-a',
  mailboxId: 'mailbox-a',
  threadId: 'thread-1',
  messageId: '<message-1@example.com>',
  subject: '第一封邮件',
  normalizedSubject: '第一封邮件',
  from: { name: 'Sender', email: 'sender@example.com' },
  to: [{ email: 'first@qq.com' }],
  date: '2026-09-03T00:00:00Z',
  preview: '预览内容',
  bodyText: '邮件正文',
  isRead: false,
  isStarred: false,
  hasAttachment: false,
  labels: [],
  ...overrides,
});

function withQueryClient(children: ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  vi.clearAllMocks();
  window.history.replaceState({}, '', '/mail');
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
  Object.defineProperty(window, 'requestAnimationFrame', {
    configurable: true,
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(0), 0),
  });

  useUiStore.setState({
    selectedMailboxId: 'inbox',
    selectedMessageId: null,
    searchOpen: false,
    composeOpen: false,
    composeDraft: null,
    navPage: 'mail',
    syncMessage: null,
  });

  apiMocks.listAccounts.mockResolvedValue([]);
  apiMocks.listMailboxes.mockResolvedValue([]);
  apiMocks.listMessages.mockResolvedValue([]);
  apiMocks.listOutbox.mockResolvedValue([]);
  apiMocks.getSettings.mockResolvedValue({
    theme: 'system',
    safeReading: true,
    syncPolicy: 'automatic',
  });
  apiMocks.searchMessages.mockResolvedValue([]);
  apiMocks.startSync.mockResolvedValue({ accountId: 'account-a', state: 'idle' });
  apiMocks.syncAll.mockResolvedValue([]);
  apiMocks.mutateMessage.mockResolvedValue(message({ isRead: true }));
  apiMocks.mutateMessages.mockResolvedValue({ mutated: 0 });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('邮箱账户关键流程', () => {
  it('QQ 授权失败时保留在凭据页、显示错误且绝不调用 onSaved', async () => {
    const onSaved = vi.fn();
    apiMocks.createAccount.mockRejectedValue({
      code: 'authentication',
      message: '客户端授权码错误，请重新输入',
      retryable: false,
    });

    render(<AccountWizard onClose={vi.fn()} onSaved={onSaved} />);

    fireEvent.click(screen.getByRole('button', { name: /QQ 邮箱/ }));
    fireEvent.change(screen.getByLabelText('邮箱地址'), { target: { value: 'broken@qq.com' } });
    fireEvent.change(screen.getByLabelText('客户端授权码'), { target: { value: 'wrong-code' } });
    fireEvent.click(screen.getByRole('button', { name: '验证并添加' }));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('客户端授权码错误，请重新输入');
    expect(screen.getByLabelText('添加邮箱，第 2 步，共 2 步')).toBeTruthy();
    expect(screen.getByLabelText('邮箱地址')).toBeTruthy();
    expect(onSaved).not.toHaveBeenCalled();
    expect(apiMocks.createAccount).toHaveBeenCalledTimes(1);
  }, 15_000);

  it('验证成功时即使验证期间重复点击也只调用一次 onSaved', async () => {
    const onSaved = vi.fn();
    const savedAccount = account({ id: 'verified-account', email: 'valid@qq.com' });
    let resolveCreate: ((value: Account) => void) | undefined;
    apiMocks.createAccount.mockImplementation(
      () =>
        new Promise<Account>((resolve) => {
          resolveCreate = resolve;
        }),
    );

    render(<AccountWizard onClose={vi.fn()} onSaved={onSaved} />);

    fireEvent.click(screen.getByRole('button', { name: /QQ 邮箱/ }));
    fireEvent.change(screen.getByLabelText('邮箱地址'), { target: { value: 'valid@qq.com' } });
    fireEvent.change(screen.getByLabelText('客户端授权码'), { target: { value: 'valid-code' } });
    const submit = screen.getByRole('button', { name: '验证并添加' });
    fireEvent.click(submit);

    await waitFor(() => expect(apiMocks.createAccount).toHaveBeenCalledTimes(1));
    expect(submit).toHaveProperty('disabled', true);
    fireEvent.click(submit);
    expect(apiMocks.createAccount).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveCreate?.(savedAccount);
    });
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(onSaved).toHaveBeenCalledWith(savedAccount);
  }, 15_000);

  it('通用 SMTP 仅验证发件端点，不请求或保存收件配置', async () => {
    const onSaved = vi.fn();
    const savedAccount = account({
      id: 'smtp-only-account',
      providerId: 'generic-smtp',
      email: 'sender@example.com',
      incomingConfigured: false,
      outgoingConfigured: true,
    });
    apiMocks.createAccount.mockResolvedValue(savedAccount);

    render(<AccountWizard onClose={vi.fn()} onSaved={onSaved} />);

    fireEvent.click(screen.getByRole('button', { name: /通用 SMTP（仅发件）/ }));
    expect(screen.queryByText('收件 IMAP')).toBeNull();
    expect(screen.getByText('发件 SMTP')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('邮箱地址'), {
      target: { value: 'sender@example.com' },
    });
    fireEvent.change(screen.getByLabelText('SMTP 密码或授权码'), {
      target: { value: 'smtp-secret' },
    });
    fireEvent.change(screen.getByLabelText('服务器'), { target: { value: 'smtp.example.com' } });
    fireEvent.change(screen.getByLabelText('端口'), { target: { value: '587' } });
    fireEvent.change(screen.getByLabelText('用户名（可选）'), { target: { value: 'smtp-user' } });
    fireEvent.change(screen.getByLabelText('TLS 模式'), { target: { value: 'starttls' } });
    fireEvent.click(screen.getByRole('button', { name: '验证并添加' }));

    await waitFor(() =>
      expect(apiMocks.createAccount).toHaveBeenCalledWith({
        email: 'sender@example.com',
        displayName: 'sender',
        providerId: 'generic-smtp',
        secret: 'smtp-secret',
        incomingSecret: undefined,
        outgoingSecret: 'smtp-secret',
        incoming: undefined,
        outgoing: {
          protocol: 'smtp',
          host: 'smtp.example.com',
          port: 587,
          tlsMode: 'starttls',
          authMethod: 'password',
          username: 'smtp-user',
        },
      }),
    );
    expect(onSaved).toHaveBeenCalledWith(savedAccount);
  }, 15_000);

  it('零账户启动时显示主界面空状态与设置入口，不强制弹出添加邮箱', async () => {
    window.history.replaceState({}, '', '/settings');
    render(<App />);

    expect(await screen.findByRole('heading', { name: '尚未添加邮箱' })).toBeTruthy();
    expect(window.location.pathname).toBe('/mail');
    expect(screen.queryByRole('dialog', { name: '添加邮箱' })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: '打开设置' }));
    expect(await screen.findByRole('heading', { name: '邮箱账户' })).toBeTruthy();
    expect(window.location.pathname).toBe('/settings');
  }, 15_000);

  it('多账户撰写会把用户选择的发件账户传给 sendDraft', async () => {
    const accounts = [
      account(),
      account({
        id: 'account-b',
        providerId: 'netease-163',
        email: 'second@163.com',
        displayName: '第二个账户',
      }),
    ];
    apiMocks.sendDraft.mockResolvedValue({ outboxId: 'outbox-1', state: 'queued' });

    render(
      withQueryClient(
        <ComposeDialog accounts={accounts} defaultAccountId="account-a" onClose={vi.fn()} />,
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: '添加附件' }));
    expect((await screen.findByRole('alert')).textContent).toContain(
      '附件选择需要在已安装的 Mutsumi Mail 客户端中完成。',
    );

    fireEvent.change(screen.getByLabelText('选择发件账户'), { target: { value: 'account-b' } });
    fireEvent.change(screen.getByPlaceholderText('输入一个或多个邮箱地址'), {
      target: { value: 'receiver@example.com' },
    });
    fireEvent.change(screen.getByPlaceholderText('邮件主题'), {
      target: { value: '多账户发件测试' },
    });
    fireEvent.change(screen.getByPlaceholderText('写下你的邮件内容…'), {
      target: { value: '正文' },
    });
    fireEvent.click(screen.getByRole('button', { name: '发送' }));

    await waitFor(() => expect(apiMocks.sendDraft).toHaveBeenCalledTimes(1));
    expect(apiMocks.sendDraft).toHaveBeenCalledWith(
      expect.objectContaining({
        accountId: 'account-b',
        to: 'receiver@example.com',
        subject: '多账户发件测试',
      }),
    );
  }, 15_000);

  it('安全渲染 HTML 正文、展示附件，并保留回复线程与收件账户', () => {
    const message: Message = {
      id: 'message-1',
      accountId: 'account-b',
      mailboxId: 'mailbox-b',
      threadId: 'thread-1',
      messageId: '<message-1@example.com>',
      subject: '只含 HTML 的邮件',
      normalizedSubject: '只含 html 的邮件',
      from: { name: 'Sender', email: 'sender@example.com' },
      to: [{ email: 'second@163.com' }, { email: 'colleague@example.com' }],
      date: '2026-09-03T00:00:00Z',
      preview: '短预览',
      bodyHtmlText: '<table><tbody><tr><td>完整 HTML 正文</td></tr></tbody></table><script>bad()</script>',
      isRead: true,
      isStarred: false,
      hasAttachment: true,
      attachmentCount: 1,
      attachments: [
        { id: 'attachment-1', filename: '报告.pdf', contentType: 'application/pdf', sizeBytes: 2048 },
      ],
      labels: [],
    };

    render(<Reader message={message} accountEmail="second@163.com" />);

    expect(screen.getByText('完整 HTML 正文').closest('table')).not.toBeNull();
    expect(document.querySelector('script')).toBeNull();
    expect(screen.getByText('报告.pdf')).toBeTruthy();
    expect(screen.getByRole('button', { name: '查看' })).toBeTruthy();
    expect(screen.getByRole('button', { name: /下载/ })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: '回复' }));
    expect(useUiStore.getState().composeDraft).toEqual(
      expect.objectContaining({
        accountId: 'account-b',
        inReplyTo: '<message-1@example.com>',
        references: ['<message-1@example.com>'],
        to: 'sender@example.com',
      }),
    );
  });

  it('打开阅读器后把未读邮件持久化为已读，且不会重复提交', async () => {
    const unread = message();
    apiMocks.mutateMessage.mockResolvedValue({ ...unread, isRead: true });
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
    });
    client.setQueryData(['mailboxes', 'all'], []);
    client.setQueryData(['messages', 'all', 'inbox', false], []);

    render(
      <QueryClientProvider client={client}>
        <MemoryRouter>
          <MailHome
            hasAccounts
            accounts={[account()]}
            messages={[unread]}
            mailboxes={[]}
            isLoading={false}
            onOpenSettings={vi.fn()}
          />
        </MemoryRouter>
        ,
      </QueryClientProvider>,
    );

    await waitFor(() =>
      expect(apiMocks.mutateMessage).toHaveBeenCalledWith(
        { messageId: unread.id, mailboxId: unread.mailboxId },
        { isRead: true },
      ),
    );
    expect(apiMocks.mutateMessage).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(client.getQueryState(['mailboxes', 'all'])?.isInvalidated).toBe(true);
      expect(client.getQueryState(['messages', 'all', 'inbox', false])?.isInvalidated).toBe(true);
    });
  });

  it('邮件列表支持批量标记已读和移至回收站', async () => {
    const first = message();
    const second = message({
      id: 'message-2',
      subject: '第二封邮件',
      messageId: '<message-2@example.com>',
    });
    const onBulkMutate = vi.fn().mockResolvedValue(undefined);
    const onBulkDelete = vi.fn().mockResolvedValue(undefined);

    render(
      <MemoryRouter>
        <MessageList
          accounts={[account()]}
          messages={[first, second]}
          onSelect={vi.fn()}
          onToggle={vi.fn()}
          onBulkMutate={onBulkMutate}
          onBulkDelete={onBulkDelete}
        />
      </MemoryRouter>,
    );

    expect(screen.queryByRole('button', { name: '选择 第一封邮件' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: '批量编辑' }));
    expect(screen.getByText('批量编辑')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: '选择 第一封邮件' }));
    fireEvent.click(screen.getByRole('button', { name: '选择 第二封邮件' }));
    expect(screen.getByText('2 已选')).toBeTruthy();

    const toolbar = screen.getByText('2 已选').closest('.list-toolbar');
    expect(toolbar).not.toBeNull();
    fireEvent.click(within(toolbar as HTMLElement).getByRole('button', { name: '标记为已读' }));
    await waitFor(() =>
      expect(onBulkMutate).toHaveBeenCalledWith([first, second], { isRead: true }),
    );

    fireEvent.click(within(toolbar as HTMLElement).getByRole('button', { name: '移至回收站' }));
    await waitFor(() => expect(onBulkDelete).toHaveBeenCalledWith([first, second]));
    await waitFor(() => expect(screen.queryByText('2 已选')).toBeNull());
  });

});

it('发送入队后禁用重复发送，编辑中的关闭需明确丢弃', async () => {
  const onClose = vi.fn();
  apiMocks.sendDraft.mockResolvedValue({ outboxId: 'queued-once', state: 'queued' });
  render(withQueryClient(<ComposeDialog accounts={[account()]} onClose={onClose} />));
  fireEvent.change(screen.getByPlaceholderText('输入一个或多个邮箱地址'), { target: { value: 'a@example.com' } });
  fireEvent.change(screen.getByPlaceholderText('邮件主题'), { target: { value: '只发送一次' } });
  fireEvent.click(screen.getByRole('button', { name: '关闭撰写' }));
  expect(onClose).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '继续编辑' }));
  fireEvent.click(screen.getByRole('button', { name: '发送' }));
  await screen.findByText('邮件已加入发件队列');
  const send = screen.getByRole('button', { name: '发送' }) as HTMLButtonElement;
  expect(send.disabled).toBe(true);
  fireEvent.click(send);
  expect(apiMocks.sendDraft).toHaveBeenCalledTimes(1);
});

it('自动保存未完成时发送等待同一草稿，避免重新创建已发送草稿', async () => {
  let finishSave!: (value: { id: string }) => void;
  apiMocks.saveDraft.mockImplementation(() => new Promise((resolve) => { finishSave = resolve; }));
  apiMocks.sendDraft.mockResolvedValue({ outboxId: 'serialized', state: 'queued' });
  render(withQueryClient(<ComposeDialog accounts={[account()]} onClose={vi.fn()} />));
  fireEvent.change(screen.getByPlaceholderText('输入一个或多个邮箱地址'), { target: { value: 'a@example.com' } });
  fireEvent.change(screen.getByPlaceholderText('邮件主题'), { target: { value: '等待保存' } });
  await waitFor(() => expect(apiMocks.saveDraft).toHaveBeenCalledTimes(1), { timeout: 2500 });
  const input = apiMocks.saveDraft.mock.calls[0][0];
  fireEvent.click(screen.getByRole('button', { name: '发送' }));
  await act(async () => { await Promise.resolve(); });
  expect(apiMocks.sendDraft).not.toHaveBeenCalled();
  await act(async () => finishSave({ id: input.id }));
  await waitFor(() => expect(apiMocks.sendDraft).toHaveBeenCalledWith(expect.objectContaining({ id: input.id })));
});

it('正文下载完成不能覆盖后来同步的新星标状态', async () => {
  const original = message({ isRead: true, bodyText: undefined });
  apiMocks.fetchMessageBody.mockResolvedValue({ ...original, bodyText: '已下载正文' });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = (item: Message) => <QueryClientProvider client={client}><MemoryRouter><MailHome hasAccounts accounts={[account()]} messages={[item]} mailboxes={[]} isLoading={false} onOpenSettings={vi.fn()} /></MemoryRouter></QueryClientProvider>;
  const { rerender } = render(view(original));
  await screen.findByText('已下载正文');
  rerender(view({ ...original, isStarred: true }));
  expect(within(screen.getByRole('article')).getByRole('button', { name: '取消星标' })).toBeTruthy();
});
