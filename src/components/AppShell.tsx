import { useEffect, type ReactNode } from 'react';
import { NavLink, useNavigate } from 'react-router-dom';
import type { Account, Mailbox } from '../types';
import { Icon } from '../lib/icons';
import { PRODUCT } from '../lib/product';
import { useUiStore } from '../stores/ui';

interface AppShellProps {
  account?: Account;
  mailboxes: Mailbox[];
  messageCount: number;
  onAddAccount: () => void;
  children: ReactNode;
}

const roleIcon = (role?: Mailbox['specialRole']) => {
  if (role === 'inbox') return 'inbox';
  if (role === 'sent') return 'send';
  if (role === 'drafts') return 'draft';
  if (role === 'trash') return 'trash';
  if (role === 'archive') return 'archive';
  return 'folder';
};

export function AppShell({ account, mailboxes, messageCount, onAddAccount, children }: AppShellProps) {
  const navigate = useNavigate();
  const { isOffline, toggleOffline, themeMode, setThemeMode, selectedMailboxId, selectMailbox, setNavPage, navPage, syncMessage } = useUiStore();

  useEffect(() => {
    const applyTheme = () => {
      const resolved = themeMode === 'system'
        ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
        : themeMode;
      document.documentElement.dataset.theme = resolved;
    };
    applyTheme();
    if (themeMode !== 'system') return undefined;
    const media = window.matchMedia('(prefers-color-scheme: light)');
    media.addEventListener('change', applyTheme);
    return () => media.removeEventListener('change', applyTheme);
  }, [themeMode]);

  return (
    <div className="app-shell">
      <aside className="sidebar" aria-label="邮箱导航">
        <div className="brand-lockup">
          <div className="brand-mark">m</div>
          <div>
            <div className="brand-name">{PRODUCT.name}</div>
            <div className="brand-tagline">{PRODUCT.tagline}</div>
          </div>
        </div>

        <div className="account-switcher">
          <div className="avatar avatar-primary">{account?.displayName?.slice(0, 1) ?? '小'}</div>
          <div className="account-copy">
            <span className="account-name">{account?.displayName ?? '本地演示账户'}</span>
            <span className="account-address">{account?.email ?? '添加一个邮箱开始'}</span>
          </div>
          <button className="icon-button subtle" onClick={onAddAccount} aria-label="添加或切换账户" title="添加或切换账户"><Icon name="chevron" size={16} /></button>
        </div>
        <button className="mobile-add-account icon-button" onClick={onAddAccount} aria-label="添加账户" title="添加账户"><Icon name="plus" size={18} /></button>

        <nav className="primary-nav">
          <NavLink className="nav-item" to="/mail" onClick={() => setNavPage('mail')}><Icon name="inbox" size={19} /><span>统一收件箱</span><span className="nav-count">{messageCount}</span></NavLink>
          <button className="nav-item" onClick={() => { setNavPage('mail'); selectMailbox('starred'); }}><Icon name="star" size={19} /><span>星标邮件</span></button>
          <NavLink className="nav-item" to="/outbox" onClick={() => setNavPage('outbox')}><Icon name="sendClock" size={19} /><span>发件箱</span><span className="nav-status-dot" /></NavLink>
        </nav>

        <div className="section-label-row"><span>文件夹</span><button className="mini-button" aria-label="新建文件夹"><Icon name="plus" size={16} /></button></div>
        <nav className="folder-nav">
          {mailboxes.filter((mailbox) => mailbox.id !== 'starred').map((mailbox) => (
            <button key={mailbox.id} className={`nav-item ${selectedMailboxId === mailbox.id ? 'is-active' : ''}`} onClick={() => { selectMailbox(mailbox.id); navigate('/mail'); }}>
              <Icon name={roleIcon(mailbox.specialRole)} size={19} />
              <span>{mailbox.displayName}</span>
              {mailbox.unreadCount > 0 && <span className="nav-count">{mailbox.unreadCount}</span>}
            </button>
          ))}
        </nav>

        <div className="sidebar-spacer" />
        <button className="compose-button" onClick={() => useUiStore.getState().setComposeOpen(true)}><span className="compose-icon"><Icon name="pen" size={19} /></span><span>撰写邮件</span></button>
        <div className="sidebar-bottom">
          <button className="nav-item" onClick={() => { setNavPage('settings'); navigate('/settings'); }}><Icon name="settings" size={19} /><span>设置</span></button>
          <button className="storage-meter" onClick={() => { setNavPage('settings'); navigate('/settings'); }}><span><span className="meter-dot" />本地缓存</span><strong>12.4 MB</strong></button>
        </div>
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div className="topbar-title"><span className="mobile-brand-mark">m</span><span className="topbar-page-title">{navPage === 'settings' ? '设置' : navPage === 'outbox' ? '发件箱' : navPage === 'search' ? '搜索' : '收件箱'}</span></div>
          <div className="topbar-actions">
            <button className={`sync-chip ${isOffline ? 'is-offline' : ''}`} onClick={toggleOffline} title="点击模拟网络状态"><span className="status-pulse" />{isOffline ? '离线 · 已缓存' : syncMessage ?? '已同步 · 2 分钟前'}</button>
            <button className="icon-button" onClick={() => { setThemeMode(themeMode === 'dark' ? 'light' : 'dark'); }} aria-label="切换主题" title="切换浅色/深色"><Icon name={themeMode === 'dark' ? 'sun' : 'moon'} size={19} /></button>
            <button className="icon-button" onClick={() => navigate('/settings')} aria-label="更多设置"><Icon name="more" size={20} /></button>
          </div>
        </header>
        <div className="workspace-content">{children}</div>
      </main>
    </div>
  );
}
