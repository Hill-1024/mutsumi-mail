import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type RefObject,
} from 'react';
import { NavLink, useLocation, useNavigate } from 'react-router-dom';
import type { Account, Mailbox } from '../types';
import { Icon, type IconName } from '../lib/icons';
import { useUiStore } from '../stores/ui';

interface AppShellProps {
  accounts: Account[];
  selectedAccountId: string | null;
  mailboxes: Mailbox[];
  messageCount: number;
  onSelectAccount: (accountId: string | null) => void;
  onAddAccount: () => void;
  children: ReactNode;
}

const roleIcon = (role?: Mailbox['specialRole'], filled = false): IconName => {
  if (role === 'inbox') return filled ? 'inboxFilled' : 'inbox';
  if (role === 'sent') return filled ? 'sendFilled' : 'send';
  if (role === 'drafts') return filled ? 'draftFilled' : 'draft';
  if (role === 'trash') return filled ? 'trashFilled' : 'trash';
  if (role === 'archive') return filled ? 'archiveFilled' : 'archive';
  if (role === 'all') return filled ? 'archiveFilled' : 'archive';
  if (role === 'starred') return filled ? 'starFilled' : 'star';
  return filled ? 'folderFilled' : 'folder';
};

const accountStatusLabel = (account: Account) => {
  if (!account.enabled) return '已停用';
  if (account.syncStatus === 'syncing') return '正在同步';
  if (account.syncStatus === 'offline') return '离线';
  if (account.syncStatus === 'error') return '同步失败';
  return '已连接';
};

interface AccountMenuProps {
  id: string;
  accounts: Account[];
  selectedAccountId: string | null;
  menuRef: RefObject<HTMLDivElement | null>;
  surface: 'desktop' | 'mobile';
  onSelectAccount: (accountId: string | null) => void;
  onAddAccount: () => void;
}

function AccountMenu({
  id,
  accounts,
  selectedAccountId,
  menuRef,
  surface,
  onSelectAccount,
  onAddAccount,
}: AccountMenuProps) {
  return (
    <div
      ref={menuRef}
      id={id}
      className={`account-menu account-menu-${surface}`}
      role="menu"
      aria-label="选择邮箱范围"
      onKeyDown={(event) => {
        if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
        const items = Array.from(
          event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'),
        );
        if (items.length === 0) return;
        event.preventDefault();
        const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
        if (event.key === 'Home') items[0].focus();
        else if (event.key === 'End') items[items.length - 1].focus();
        else if (event.key === 'ArrowDown') items[(currentIndex + 1) % items.length].focus();
        else items[(currentIndex - 1 + items.length) % items.length].focus();
      }}
    >
      <span className="account-menu-label">查看范围</span>
      <button
        className={`account-menu-item ${selectedAccountId === null ? 'is-selected' : ''}`}
        type="button"
        role="menuitemradio"
        aria-checked={selectedAccountId === null}
        disabled={accounts.length === 0}
        onClick={() => onSelectAccount(null)}
      >
        <span className="avatar account-menu-avatar">
          <Icon name="inbox" size={18} />
        </span>
        <span className="account-menu-copy">
          <strong>所有收件箱</strong>
          <span>{accounts.length > 0 ? `同时查看 ${accounts.length} 个账户` : '请先添加邮箱'}</span>
        </span>
        {selectedAccountId === null && accounts.length > 0 ? <Icon name="check" size={18} /> : null}
      </button>

      {accounts.map((account) => {
        const selected = account.id === selectedAccountId;
        return (
          <button
            key={account.id}
            className={`account-menu-item ${selected ? 'is-selected' : ''}`}
            type="button"
            role="menuitemradio"
            aria-checked={selected}
            onClick={() => onSelectAccount(account.id)}
          >
            <span className="avatar avatar-primary">
              {account.displayName.trim().slice(0, 1) || account.email.slice(0, 1).toUpperCase()}
            </span>
            <span className="account-menu-copy">
              <strong>{account.displayName || account.email}</strong>
              <span>
                {account.email} · {accountStatusLabel(account)}
              </span>
            </span>
            {selected ? <Icon name="check" size={18} /> : null}
          </button>
        );
      })}

      <span className="account-menu-divider" role="separator" />
      <button
        className="account-menu-item account-menu-add"
        type="button"
        role="menuitem"
        onClick={onAddAccount}
      >
        <span className="avatar account-menu-avatar">
          <Icon name="plus" size={18} />
        </span>
        <span className="account-menu-copy">
          <strong>添加邮箱</strong>
        </span>
      </button>
    </div>
  );
}

export function AppShell({
  accounts,
  selectedAccountId,
  mailboxes,
  messageCount,
  onSelectAccount,
  onAddAccount,
  children,
}: AppShellProps) {
  const navigate = useNavigate();
  const location = useLocation();
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false);
  const [accountMenuSurface, setAccountMenuSurface] = useState<'desktop' | 'mobile' | null>(null);
  const {
    themeMode,
    setThemeMode,
    selectedMailboxId,
    selectedMessageId,
    selectMailbox,
    setNavPage,
    navPage,
    syncMessage,
    composeOpen,
    setComposeOpen,
    setSearchOpen,
  } = useUiStore();
  const selectedAccount = selectedAccountId
    ? accounts.find((candidate) => candidate.id === selectedAccountId)
    : undefined;
  const canCompose = accounts.some(
    (candidate) => candidate.enabled && candidate.outgoingConfigured,
  );
  const inboxId = 'inbox';
  const scopeName = selectedAccount
    ? selectedAccount.displayName || selectedAccount.email
    : accounts.length > 0
      ? '所有收件箱'
      : '添加邮箱';
  const scopeDescription =
    selectedAccount?.email ??
    (accounts.length > 0 ? `${accounts.length} 个邮箱账户` : '尚未配置账户');
  const drawerRef = useRef<HTMLElement>(null);
  const drawerTriggerRef = useRef<HTMLButtonElement | null>(null);
  const restoreDrawerFocusRef = useRef(true);
  const desktopAccountTriggerRef = useRef<HTMLButtonElement | null>(null);
  const mobileAccountTriggerRef = useRef<HTMLButtonElement | null>(null);
  const desktopAccountMenuRef = useRef<HTMLDivElement | null>(null);
  const mobileAccountMenuRef = useRef<HTMLDivElement | null>(null);
  const openMobileMenu = (event: ReactMouseEvent<HTMLButtonElement>) => {
    drawerTriggerRef.current = event.currentTarget;
    restoreDrawerFocusRef.current = true;
    setAccountMenuSurface(null);
    setMobileMenuOpen(true);
  };
  const closeMobileMenu = () => {
    setAccountMenuSurface(null);
    setMobileMenuOpen(false);
  };
  const openMailbox = (mailboxId: string) => {
    selectMailbox(mailboxId);
    setAccountMenuSurface(null);
    setMobileMenuOpen(false);
    navigate('/mail');
  };
  const openPage = (page: 'outbox' | 'settings') => {
    setNavPage(page);
    setAccountMenuSurface(null);
    setMobileMenuOpen(false);
    navigate(`/${page}`);
  };
  const openAccountMenu = (surface: 'desktop' | 'mobile') => {
    setAccountMenuSurface((current) => (current === surface ? null : surface));
  };
  const selectAccountScope = (accountId: string | null) => {
    onSelectAccount(accountId);
    selectMailbox('inbox');
    setNavPage('mail');
    setAccountMenuSurface(null);
    setMobileMenuOpen(false);
    navigate('/mail');
  };
  const addAccount = () => {
    setAccountMenuSurface(null);
    if (mobileMenuOpen) {
      restoreDrawerFocusRef.current = false;
      drawerTriggerRef.current = null;
      setMobileMenuOpen(false);
    }
    onAddAccount();
  };
  const folderGroups = useMemo(() => {
    if (selectedAccountId) {
      return [
        {
          accountId: selectedAccountId,
          label: null,
          mailboxes: mailboxes.filter(
            (mailbox) => mailbox.accountId === selectedAccountId && mailbox.specialRole !== 'inbox',
          ),
        },
      ];
    }

    return accounts
      .map((candidate) => ({
        accountId: candidate.id,
        label: accounts.length > 1 ? candidate.email : null,
        mailboxes: mailboxes.filter(
          (mailbox) => mailbox.accountId === candidate.id && mailbox.specialRole !== 'inbox',
        ),
      }))
      .filter((group) => group.mailboxes.length > 0);
  }, [accounts, mailboxes, selectedAccountId]);
  const syncLabel = (() => {
    if (accounts.length === 0) return '未配置账户';
    if (syncMessage) return syncMessage;
    const scopedAccounts = selectedAccount ? [selectedAccount] : accounts;
    if (scopedAccounts.some((candidate) => candidate.syncStatus === 'syncing')) return '正在同步';
    const problemCount = scopedAccounts.filter(
      (candidate) => candidate.syncStatus === 'offline' || candidate.syncStatus === 'error',
    ).length;
    if (problemCount > 0)
      return selectedAccount
        ? accountStatusLabel(selectedAccount)
        : `${problemCount} 个账户同步异常`;
    const lastSyncedAt = scopedAccounts
      .map((candidate) => candidate.lastSyncedAt)
      .filter((value): value is string => Boolean(value))
      .sort()
      .at(-1);
    if (!lastSyncedAt) return '等待首次同步';
    const syncedAt = new Date(lastSyncedAt);
    return Number.isNaN(syncedAt.getTime())
      ? '已同步'
      : `上次同步 ${syncedAt.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}`;
  })();

  // Synchronize route changes with navPage
  useEffect(() => {
    const path = location.pathname;
    if (path.startsWith('/settings')) setNavPage('settings');
    else if (path.startsWith('/outbox')) setNavPage('outbox');
    else if (path.startsWith('/search')) setNavPage('search');
    else if (path.startsWith('/mail')) setNavPage('mail');
  }, [location.pathname, setNavPage]);

  useEffect(() => {
    const closeOnHistoryNavigation = () => {
      setAccountMenuSurface(null);
      setMobileMenuOpen(false);
    };
    window.addEventListener('popstate', closeOnHistoryNavigation);
    return () => window.removeEventListener('popstate', closeOnHistoryNavigation);
  }, []);

  // Handle theme changes
  useEffect(() => {
    const applyTheme = () => {
      const resolved =
        themeMode === 'system'
          ? window.matchMedia('(prefers-color-scheme: light)').matches
            ? 'light'
            : 'dark'
          : themeMode;
      document.documentElement.dataset.theme = resolved;
    };
    applyTheme();
    if (themeMode !== 'system') return undefined;
    const media = window.matchMedia('(prefers-color-scheme: light)');
    media.addEventListener('change', applyTheme);
    return () => media.removeEventListener('change', applyTheme);
  }, [themeMode]);

  useEffect(() => {
    if (!accountMenuSurface) return undefined;

    const menu =
      accountMenuSurface === 'desktop'
        ? desktopAccountMenuRef.current
        : mobileAccountMenuRef.current;
    const trigger =
      accountMenuSurface === 'desktop'
        ? desktopAccountTriggerRef.current
        : mobileAccountTriggerRef.current;
    menu?.querySelector<HTMLElement>('button:not(:disabled)')?.focus();

    const closeWhenClickingOutside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (menu?.contains(target) || trigger?.contains(target)) return;
      setAccountMenuSurface(null);
    };

    window.addEventListener('pointerdown', closeWhenClickingOutside);
    return () => window.removeEventListener('pointerdown', closeWhenClickingOutside);
  }, [accountMenuSurface]);

  useEffect(() => {
    if (!mobileMenuOpen) return undefined;

    const getFocusable = () =>
      drawerRef.current?.querySelectorAll<HTMLElement>(
        'button:not(:disabled), a[href], input, textarea, [tabindex]:not([tabindex="-1"])',
      );
    getFocusable()?.[0]?.focus();

    const trapFocus = (event: KeyboardEvent) => {
      const focusable = getFocusable();
      if (event.key !== 'Tab' || !focusable?.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    window.addEventListener('keydown', trapFocus);
    return () => {
      window.removeEventListener('keydown', trapFocus);
      if (!restoreDrawerFocusRef.current) return;
      const trigger = drawerTriggerRef.current;
      if (trigger?.isConnected && trigger.getClientRects().length > 0) trigger.focus();
      else document.querySelector<HTMLElement>('.topbar-page-title')?.focus();
    };
  }, [mobileMenuOpen]);

  useEffect(() => {
    const largeWindow = window.matchMedia('(min-width: 1200px)');
    const closeAtLargeSize = (event: MediaQueryListEvent) => {
      if (event.matches) setMobileMenuOpen(false);
    };
    largeWindow.addEventListener('change', closeAtLargeSize);
    return () => largeWindow.removeEventListener('change', closeAtLargeSize);
  }, []);

  // Global keyboard shortcuts (Cmd+K, Esc, C)
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (accountMenuSurface) {
        if (event.key === 'Escape') {
          event.preventDefault();
          setAccountMenuSurface(null);
          const trigger =
            accountMenuSurface === 'desktop'
              ? desktopAccountTriggerRef.current
              : mobileAccountTriggerRef.current;
          trigger?.focus();
        }
        return;
      }

      if (mobileMenuOpen) {
        if (event.key === 'Escape') {
          event.preventDefault();
          setMobileMenuOpen(false);
        }
        return;
      }

      // ⌘K or Ctrl+K to search
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setNavPage('search');
        setSearchOpen(true);
        navigate('/search');
        return;
      }

      // Escape to close modals or return from search
      if (event.key === 'Escape') {
        if (composeOpen) {
          setComposeOpen(false);
          return;
        }
        if (location.pathname === '/search') {
          setNavPage('mail');
          navigate('/mail');
          return;
        }
      }

      // Quick key 'c' or 'C' to compose (when not in inputs/textareas)
      const targetTag = (event.target as HTMLElement)?.tagName?.toLowerCase();
      const isInput =
        targetTag === 'input' ||
        targetTag === 'textarea' ||
        (event.target as HTMLElement)?.isContentEditable;
      if (
        canCompose &&
        !isInput &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        event.key.toLowerCase() === 'c'
      ) {
        event.preventDefault();
        setComposeOpen(true);
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [
    accountMenuSurface,
    canCompose,
    composeOpen,
    location.pathname,
    mobileMenuOpen,
    navigate,
    setComposeOpen,
    setNavPage,
    setSearchOpen,
  ]);

  // Dynamic topbar title
  const currentTitle = useMemo(() => {
    if (navPage === 'settings') return '设置';
    if (navPage === 'outbox') return '发件箱';
    if (navPage === 'search') return '搜索邮件';
    if (selectedMailboxId === 'starred') return '星标邮件';
    if (selectedMailboxId === 'inbox')
      return selectedAccount
        ? `${selectedAccount.displayName || selectedAccount.email} · 收件箱`
        : '所有收件箱';
    const currentMailbox = mailboxes.find((m) => m.id === selectedMailboxId);
    return currentMailbox?.displayName ?? '收件箱';
  }, [navPage, selectedAccount, selectedMailboxId, mailboxes]);

  return (
    <div
      className={`app-shell ${navPage === 'mail' && selectedMessageId ? 'is-reading-message' : ''}`}
    >
      <aside className="sidebar" aria-label="邮箱导航" inert={mobileMenuOpen ? true : undefined}>
        <button
          className="rail-menu-trigger icon-button"
          type="button"
          onClick={openMobileMenu}
          aria-label="打开账户与文件夹"
          aria-expanded={mobileMenuOpen}
          aria-controls="mobile-folder-drawer"
        >
          <Icon name="menu" size={22} />
        </button>
        <button
          className="compose-button"
          disabled={!canCompose}
          onClick={() => setComposeOpen(true)}
          aria-label="撰写邮件"
          title={canCompose ? '撰写新邮件 (快捷键 C)' : '没有可用的发件账户'}
        >
          <span className="compose-icon">
            <Icon name="pen" size={20} />
          </span>
          <span className="compose-label">撰写邮件</span>
        </button>

        <nav className="primary-nav" aria-label="核心邮箱视图">
          <button
            className={`nav-item ${navPage === 'mail' && selectedMailboxId === inboxId ? 'is-active' : ''}`}
            aria-label={selectedAccount ? `${selectedAccount.displayName} 收件箱` : '所有收件箱'}
            aria-current={navPage === 'mail' && selectedMailboxId === inboxId ? 'page' : undefined}
            onClick={() => openMailbox(inboxId)}
          >
            <Icon
              name={navPage === 'mail' && selectedMailboxId === inboxId ? 'inboxFilled' : 'inbox'}
              size={20}
            />
            <span>{selectedAccount ? '收件箱' : '所有收件箱'}</span>
            {messageCount > 0 && <span className="nav-count">{messageCount}</span>}
          </button>
          <button
            className={`nav-item ${navPage === 'mail' && selectedMailboxId === 'starred' ? 'is-active' : ''}`}
            aria-label="星标邮件"
            aria-current={
              navPage === 'mail' && selectedMailboxId === 'starred' ? 'page' : undefined
            }
            disabled={accounts.length === 0}
            onClick={() => openMailbox('starred')}
          >
            <Icon
              name={navPage === 'mail' && selectedMailboxId === 'starred' ? 'starFilled' : 'star'}
              size={20}
            />
            <span>星标邮件</span>
          </button>
          <NavLink
            className={({ isActive }) => `nav-item ${isActive ? 'is-active' : ''}`}
            to="/outbox"
            aria-label="发件箱"
            onClick={() => {
              setNavPage('outbox');
              setMobileMenuOpen(false);
            }}
          >
            <Icon
              name={location.pathname.startsWith('/outbox') ? 'sendClockFilled' : 'sendClock'}
              size={20}
            />
            <span>发件箱</span>
          </NavLink>
        </nav>

        <nav className="folder-nav" aria-label="文件夹列表">
          {folderGroups.map((group) => (
            <div className="folder-account-group" key={group.accountId}>
              {group.label ? <span className="folder-account-label">{group.label}</span> : null}
              {group.mailboxes.map((mailbox) => (
                <button
                  key={mailbox.id}
                  className={`nav-item ${navPage === 'mail' && selectedMailboxId === mailbox.id ? 'is-active' : ''}`}
                  aria-label={
                    group.label ? `${mailbox.displayName}，${group.label}` : mailbox.displayName
                  }
                  aria-current={
                    navPage === 'mail' && selectedMailboxId === mailbox.id ? 'page' : undefined
                  }
                  onClick={() => openMailbox(mailbox.id)}
                >
                  <Icon
                    name={roleIcon(
                      mailbox.specialRole,
                      navPage === 'mail' && selectedMailboxId === mailbox.id,
                    )}
                    size={20}
                  />
                  <span>{mailbox.displayName}</span>
                  {mailbox.unreadCount > 0 && (
                    <span className="nav-count">{mailbox.unreadCount}</span>
                  )}
                </button>
              ))}
            </div>
          ))}
        </nav>

        <div className="sidebar-spacer" />
        <div className="sidebar-bottom">
          <button
            className={`nav-item ${navPage === 'settings' ? 'is-active' : ''}`}
            aria-label="设置"
            aria-current={navPage === 'settings' ? 'page' : undefined}
            onClick={() => openPage('settings')}
          >
            <Icon name={navPage === 'settings' ? 'settingsFilled' : 'settings'} size={20} />
            <span>设置</span>
          </button>
        </div>
        <button
          ref={desktopAccountTriggerRef}
          className="account-switcher"
          type="button"
          aria-label="添加或切换账户"
          aria-haspopup="menu"
          aria-expanded={accountMenuSurface === 'desktop'}
          aria-controls="desktop-account-menu"
          onClick={() => openAccountMenu('desktop')}
        >
          <div className="avatar avatar-primary">
            {selectedAccount ? (
              selectedAccount.displayName.trim().slice(0, 1) ||
              selectedAccount.email.slice(0, 1).toUpperCase()
            ) : accounts.length > 0 ? (
              <Icon name="inbox" size={18} />
            ) : (
              <Icon name="plus" size={18} />
            )}
          </div>
          <div className="account-copy">
            <span className="account-name">{scopeName}</span>
            <span className="account-address">{scopeDescription}</span>
          </div>
          <Icon name="chevron" size={16} className="account-chevron" />
        </button>
        {accountMenuSurface === 'desktop' ? (
          <AccountMenu
            id="desktop-account-menu"
            accounts={accounts}
            selectedAccountId={selectedAccountId}
            menuRef={desktopAccountMenuRef}
            surface="desktop"
            onSelectAccount={selectAccountScope}
            onAddAccount={addAccount}
          />
        ) : null}
      </aside>

      <main className="workspace" inert={mobileMenuOpen ? true : undefined}>
        <header className="topbar">
          <div className="topbar-title">
            <button
              className="mobile-menu-trigger icon-button"
              type="button"
              onClick={openMobileMenu}
              aria-label="打开文件夹导航"
              aria-expanded={mobileMenuOpen}
              aria-controls="mobile-folder-drawer"
            >
              <Icon name="menu" size={22} />
            </button>
            <h1 className="topbar-page-title" tabIndex={-1}>
              {currentTitle}
            </h1>
          </div>
          <div className="topbar-actions">
            <div
              className={`sync-chip ${accounts.length === 0 ? 'is-unconfigured' : (selectedAccount ? ['offline', 'error'].includes(selectedAccount.syncStatus) : accounts.some((candidate) => candidate.syncStatus === 'offline' || candidate.syncStatus === 'error')) ? 'is-offline' : ''}`}
              role="status"
              aria-live="polite"
            >
              <span className="status-pulse" />
              <span className="sync-chip-text">{syncLabel}</span>
            </div>
            <button
              className="icon-button topbar-theme-action"
              onClick={() => {
                setThemeMode(themeMode === 'dark' ? 'light' : 'dark');
              }}
              aria-label="切换主题"
              title={themeMode === 'dark' ? '切换至浅色模式' : '切换至深色模式'}
            >
              <Icon name={themeMode === 'dark' ? 'sun' : 'moon'} size={20} />
            </button>
            <button
              className="icon-button"
              onClick={() => {
                setNavPage('search');
                setSearchOpen(true);
                navigate('/search');
              }}
              aria-label="全局搜索 (⌘K)"
              title="搜索 (⌘K)"
            >
              <Icon name="search" size={20} />
            </button>
          </div>
        </header>
        <div className="workspace-content">{children}</div>
      </main>

      {navPage === 'mail' && !selectedMessageId && (
        <button
          className="mobile-compose-fab"
          type="button"
          disabled={!canCompose}
          inert={mobileMenuOpen ? true : undefined}
          onClick={() => setComposeOpen(true)}
          aria-label="撰写邮件"
        >
          <Icon name="pen" size={24} />
        </button>
      )}

      <nav
        className="mobile-bottom-nav"
        aria-label="主要导航"
        inert={mobileMenuOpen ? true : undefined}
      >
        <button
          className={`mobile-bottom-item ${navPage === 'mail' && selectedMailboxId === inboxId ? 'is-active' : ''}`}
          type="button"
          aria-current={navPage === 'mail' && selectedMailboxId === inboxId ? 'page' : undefined}
          onClick={() => openMailbox(inboxId)}
        >
          <span className="mobile-bottom-indicator">
            <Icon
              name={navPage === 'mail' && selectedMailboxId === inboxId ? 'inboxFilled' : 'inbox'}
              size={22}
            />
          </span>
          <span>收件箱</span>
        </button>
        <button
          className={`mobile-bottom-item ${navPage === 'mail' && selectedMailboxId === 'starred' ? 'is-active' : ''}`}
          type="button"
          aria-current={navPage === 'mail' && selectedMailboxId === 'starred' ? 'page' : undefined}
          disabled={accounts.length === 0}
          onClick={() => openMailbox('starred')}
        >
          <span className="mobile-bottom-indicator">
            <Icon
              name={navPage === 'mail' && selectedMailboxId === 'starred' ? 'starFilled' : 'star'}
              size={22}
            />
          </span>
          <span>星标</span>
        </button>
        <button
          className={`mobile-bottom-item ${navPage === 'outbox' ? 'is-active' : ''}`}
          type="button"
          aria-current={navPage === 'outbox' ? 'page' : undefined}
          onClick={() => openPage('outbox')}
        >
          <span className="mobile-bottom-indicator">
            <Icon name={navPage === 'outbox' ? 'sendClockFilled' : 'sendClock'} size={22} />
          </span>
          <span>发件箱</span>
        </button>
        <button
          className={`mobile-bottom-item ${navPage === 'settings' ? 'is-active' : ''}`}
          type="button"
          aria-current={navPage === 'settings' ? 'page' : undefined}
          onClick={() => openPage('settings')}
        >
          <span className="mobile-bottom-indicator">
            <Icon name={navPage === 'settings' ? 'settingsFilled' : 'settings'} size={22} />
          </span>
          <span>设置</span>
        </button>
      </nav>

      {mobileMenuOpen && (
        <div
          className="mobile-nav-scrim"
          role="presentation"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) closeMobileMenu();
          }}
        >
          <aside
            ref={drawerRef}
            id="mobile-folder-drawer"
            className="mobile-nav-drawer"
            role="dialog"
            aria-modal="true"
            aria-label="账户与文件夹"
          >
            <div className="mobile-drawer-header">
              <div className="mobile-account-switcher-region">
                <button
                  ref={mobileAccountTriggerRef}
                  className="account-switcher"
                  type="button"
                  aria-label="添加或切换账户"
                  aria-haspopup="menu"
                  aria-expanded={accountMenuSurface === 'mobile'}
                  aria-controls="mobile-account-menu"
                  onClick={() => openAccountMenu('mobile')}
                >
                  <div className="avatar avatar-primary">
                    {selectedAccount ? (
                      selectedAccount.displayName.trim().slice(0, 1) ||
                      selectedAccount.email.slice(0, 1).toUpperCase()
                    ) : accounts.length > 0 ? (
                      <Icon name="inbox" size={18} />
                    ) : (
                      <Icon name="plus" size={18} />
                    )}
                  </div>
                  <div className="account-copy">
                    <span className="account-name">{scopeName}</span>
                    <span className="account-address">{scopeDescription}</span>
                  </div>
                  <Icon name="chevron" size={16} className="account-chevron" />
                </button>
                {accountMenuSurface === 'mobile' ? (
                  <AccountMenu
                    id="mobile-account-menu"
                    accounts={accounts}
                    selectedAccountId={selectedAccountId}
                    menuRef={mobileAccountMenuRef}
                    surface="mobile"
                    onSelectAccount={selectAccountScope}
                    onAddAccount={addAccount}
                  />
                ) : null}
              </div>
              <button
                className="icon-button"
                type="button"
                onClick={closeMobileMenu}
                aria-label="关闭文件夹导航"
              >
                <Icon name="close" size={22} />
              </button>
            </div>
            <nav className="mobile-folder-list" aria-label="邮箱文件夹">
              {folderGroups.map((group) => (
                <div className="folder-account-group" key={group.accountId}>
                  {group.label ? <span className="folder-account-label">{group.label}</span> : null}
                  {group.mailboxes.map((mailbox) => (
                    <button
                      key={mailbox.id}
                      className={`nav-item ${navPage === 'mail' && selectedMailboxId === mailbox.id ? 'is-active' : ''}`}
                      type="button"
                      aria-label={
                        group.label ? `${mailbox.displayName}，${group.label}` : mailbox.displayName
                      }
                      aria-current={
                        navPage === 'mail' && selectedMailboxId === mailbox.id ? 'page' : undefined
                      }
                      onClick={() => openMailbox(mailbox.id)}
                    >
                      <Icon
                        name={roleIcon(
                          mailbox.specialRole,
                          navPage === 'mail' && selectedMailboxId === mailbox.id,
                        )}
                        size={20}
                      />
                      <span>{mailbox.displayName}</span>
                      {mailbox.unreadCount > 0 && (
                        <span className="nav-count">{mailbox.unreadCount}</span>
                      )}
                    </button>
                  ))}
                </div>
              ))}
            </nav>
          </aside>
        </div>
      )}
    </div>
  );
}
