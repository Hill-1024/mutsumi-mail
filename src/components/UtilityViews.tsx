import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import type { Account, Message, OutboxItem } from '../types';
import { Icon } from '../lib/icons';
import { appErrorMessage, cancelOutboxItem, clearCache, getSettings, retryOutboxItem, searchMessages, updateSettings } from '../lib/tauri';
import { filterMessages } from '../lib/mail-utils';
import { useUiStore } from '../stores/ui';

export function SearchView({ messages, accountId }: { messages: Message[]; accountId?: string }) {
  const [query, setQuery] = useState('');
  const [activeFilter, setActiveFilter] = useState<'all' | 'unread' | 'starred' | 'attachment'>('all');
  const selectMessage = useUiStore((state) => state.selectMessage);
  const selectMailbox = useUiStore((state) => state.selectMailbox);
  const setNavPage = useUiStore((state) => state.setNavPage);
  const navigate = useNavigate();

  useEffect(() => {
    setNavPage('search');
  }, [setNavPage]);

  const isStructured = query.includes(':');
  const search = useQuery({
    queryKey: ['search', accountId ?? 'all', query],
    queryFn: () => searchMessages({ accountId, search: query, limit: 100 }),
    enabled: query.trim().length > 0 && !isStructured,
    staleTime: 10_000,
  });

  const baseResults = useMemo(() => {
    if (!query.trim()) return messages;
    if (isStructured) return filterMessages(messages, query);
    return search.data ?? filterMessages(messages, query);
  }, [query, messages, isStructured, search.data]);

  const filteredResults = useMemo(() => {
    if (activeFilter === 'unread') return baseResults.filter((m) => !m.isRead);
    if (activeFilter === 'starred') return baseResults.filter((m) => m.isStarred);
    if (activeFilter === 'attachment') return baseResults.filter((m) => m.hasAttachment);
    return baseResults;
  }, [baseResults, activeFilter]);

  return (
    <section className="utility-view">
      <div className="utility-header">
        <div className="large-search" role="search">
          <Icon name="search" size={22} />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="搜索邮件、发件人或主题"
            autoFocus
          />
          {query && (
            <button
              className="icon-button subtle"
              onClick={() => setQuery('')}
              aria-label="清空搜索"
              title="清空"
            >
              <Icon name="close" size={18} />
            </button>
          )}
        </div>

        <div className="search-chips-row" role="group" aria-label="快捷过滤">
          <button
            className={`m3-filter-chip ${activeFilter === 'all' ? 'is-selected' : ''}`}
            onClick={() => setActiveFilter('all')}
          >
            全部
          </button>
          <button
            className={`m3-filter-chip ${activeFilter === 'unread' ? 'is-selected' : ''}`}
            onClick={() => setActiveFilter(activeFilter === 'unread' ? 'all' : 'unread')}
          >
            <Icon name="clock" size={15} />
            未读邮件
          </button>
          <button
            className={`m3-filter-chip ${activeFilter === 'starred' ? 'is-selected' : ''}`}
            onClick={() => setActiveFilter(activeFilter === 'starred' ? 'all' : 'starred')}
          >
            <Icon name="star" size={15} />
            星标邮件
          </button>
          <button
            className={`m3-filter-chip ${activeFilter === 'attachment' ? 'is-selected' : ''}`}
            onClick={() => setActiveFilter(activeFilter === 'attachment' ? 'all' : 'attachment')}
          >
            <Icon name="paperclip" size={15} />
            包含附件
          </button>
          <span className="result-count-chip">{filteredResults.length} 个结果</span>
        </div>
      </div>

      <div className="search-result-list" role="list">
        {filteredResults.length === 0 ? (
          <div className="empty-search-state">
            <div className="empty-icon"><Icon name="search" size={28} /></div>
            <h3>未找到符合条件的邮件</h3>
            <p>可尝试检查关键词拼写，或清除过滤标签后重试。</p>
          </div>
        ) : (
          filteredResults.map((message) => (
            <button
              key={message.id}
              className="search-result"
              onClick={() => {
                selectMailbox(message.mailboxId);
                selectMessage(message.id);
                navigate('/mail');
              }}
            >
              <span className="result-avatar">
                {message.from.name?.slice(0, 1) ?? message.from.email.slice(0, 1).toUpperCase()}
              </span>
              <span className="result-copy">
                <span className="result-subject-row">
                  <strong>{message.subject || '(无主题)'}</strong>
                  {message.isStarred && <Icon name="starFilled" size={14} className="result-star" />}
                </span>
                <span>{message.from.name ?? message.from.email} · {message.preview}</span>
              </span>
              <span className="result-date">
                {new Date(message.date).toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })}
              </span>
            </button>
          ))
        )}
      </div>
    </section>
  );
}

const outboxStateLabel = (state: OutboxItem['state']) => {
  if (state === 'queued') return '等待发送';
  if (state === 'sending') return '正在发送';
  if (state === 'sent') return '已发送';
  if (state === 'failed') return '发送失败';
  if (state === 'outcome_unknown') return '结果不确定';
  return '已取消';
};

const sentCopyLabel = (state: OutboxItem['sentCopyState']) => {
  if (state === 'confirmed') return '服务器“已发送”已同步';
  if (state === 'awaiting_server_sync') return '已发送，服务器副本尚未确认';
  if (state === 'unavailable') return '该账户没有收件服务，无法同步“已发送”';
  if (state === 'failed') return '服务器“已发送”副本保存失败';
  return '';
};

export function OutboxView({ accounts, items }: { accounts: Account[]; items: OutboxItem[] }) {
  const setNavPage = useUiStore((state) => state.setNavPage);
  const queryClient = useQueryClient();
  const [pendingActionId, setPendingActionId] = useState<string | null>(null);
  const [actionError, setActionError] = useState('');
  useEffect(() => {
    setNavPage('outbox');
  }, [setNavPage]);

  const pendingCount = items.filter((item) =>
    ['queued', 'sending', 'failed', 'outcome_unknown'].includes(item.state),
  ).length;

  const runAction = async (item: OutboxItem, action: 'retry' | 'cancel') => {
    setPendingActionId(item.id);
    setActionError('');
    try {
      if (action === 'retry') await retryOutboxItem(item.id);
      else await cancelOutboxItem(item.id);
      await queryClient.invalidateQueries({ queryKey: ['outbox'] });
    } catch (error) {
      setActionError(appErrorMessage(error));
    } finally {
      setPendingActionId(null);
    }
  };

  return (
    <section className="utility-view">
      <div className="utility-toolbar">
        <span className="outbox-state">
          <span className="status-pulse" />
          {pendingCount ? `${pendingCount} 封待处理` : '没有待处理邮件'}
        </span>
      </div>
      {actionError && <div className="setting-feedback is-error" role="alert"><Icon name="close" size={16} /><span>{actionError}</span></div>}
      {items.length ? (
        <div className="outbox-list">
          {items.map((item) => (
            <div className="outbox-item" key={item.id}>
              <div className="outbox-item-icon">
                <Icon name="sendClock" size={20} />
              </div>
              <div className="outbox-item-copy">
                <strong>{item.subject || '无主题邮件'}</strong>
                <span>
                  {accounts.length > 1 ? `${accounts.find((account) => account.id === item.accountId)?.email ?? '未知账户'} · ` : ''}
                  收件人: {item.recipients.join(', ') || '未填写收件人'} ·{' '}
                  <span className={`state-badge state-${item.state}`}>
                    {outboxStateLabel(item.state)}
                  </span>
                </span>
                {item.lastErrorMessage && <span className="outbox-error" role="status">{item.lastErrorMessage}</span>}
                {item.state === 'sent' && sentCopyLabel(item.sentCopyState) && (
                  <span className={item.sentCopyState === 'failed' ? 'outbox-error' : 'outbox-copy-status'} role="status">
                    {sentCopyLabel(item.sentCopyState)}
                    {item.sentCopyErrorMessage ? `：${item.sentCopyErrorMessage}` : ''}
                  </span>
                )}
              </div>
              <div className="outbox-item-end">
                <span className="outbox-item-date">{new Date(item.updatedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</span>
                <div className="outbox-actions">
                  {(item.state === 'failed' || item.state === 'outcome_unknown') && <button className="text-action" type="button" disabled={pendingActionId === item.id} onClick={() => void runAction(item, 'retry')}>重试</button>}
                  {(item.state === 'queued' || item.state === 'failed' || item.state === 'outcome_unknown') && <button className="text-action danger-text" type="button" disabled={pendingActionId === item.id} onClick={() => void runAction(item, 'cancel')}>取消</button>}
                </div>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="outbox-empty">
          <div className="empty-icon"><Icon name="sendClock" size={32} /></div>
          <h2>没有待处理邮件</h2>
          <p>排队、失败或结果不确定的邮件会显示在这里。</p>
        </div>
      )}
    </section>
  );
}

export function SettingsView({
  accounts,
  onAddAccount,
  onRemoveAccount,
}: {
  accounts: Account[];
  onAddAccount: () => void;
  onRemoveAccount: (accountId: string) => Promise<void>;
}) {
  const {
    themeMode,
    setThemeMode,
    safeReading,
    toggleSafeReading,
    setSafeReading,
    setNavPage,
  } = useUiStore();
  const [cacheStatus, setCacheStatus] = useState('');
  const [syncPolicy, setSyncPolicy] = useState('automatic');
  const [confirmRemoveId, setConfirmRemoveId] = useState<string | null>(null);
  const [removingId, setRemovingId] = useState<string | null>(null);
  const [accountError, setAccountError] = useState('');

  useEffect(() => {
    setNavPage('settings');
    void getSettings()
      .then((settings) => {
        if (settings.theme) setThemeMode(settings.theme);
        setSafeReading(settings.safeReading);
        setSyncPolicy(settings.syncPolicy);
      })
      .catch(() => undefined);
  }, [setNavPage, setSafeReading, setThemeMode]);

  const changeTheme = (mode: 'system' | 'light' | 'dark') => {
    setThemeMode(mode);
    void updateSettings({ theme: mode }).catch(() => undefined);
  };

  const changeSafeReading = () => {
    const nextValue = !safeReading;
    toggleSafeReading();
    void updateSettings({ safeReading: nextValue }).catch(() => setSafeReading(safeReading));
  };

  const manageCache = async () => {
    setCacheStatus('正在清理本地临时缓存…');
    const result = await clearCache();
    setCacheStatus(`缓存清理完毕 · 已释放 ${result.deletedMessages} 封本地缓存邮件`);
    window.setTimeout(() => setCacheStatus(''), 4000);
  };

  const removeSelectedAccount = async (accountId: string) => {
    setRemovingId(accountId);
    setAccountError('');
    try {
      await onRemoveAccount(accountId);
      setConfirmRemoveId(null);
    } catch (error) {
      setAccountError(appErrorMessage(error));
    } finally {
      setRemovingId(null);
    }
  };

  return (
    <section className="utility-view settings-view">
      <div className="settings-grid">
        <div className="settings-section">
          <div className="settings-section-title settings-section-title-with-action">
            <div><Icon name="inbox" size={20} /><h2>邮箱账户</h2></div>
            <button className="primary-action compact" type="button" onClick={onAddAccount}>
              <Icon name="plus" size={17} />
              添加邮箱
            </button>
          </div>
          {accounts.length === 0 ? (
            <div className="settings-account-empty">
              <span>还没有邮箱账户</span>
              <button className="text-action" type="button" onClick={onAddAccount}>添加邮箱</button>
            </div>
          ) : (
            <div className="settings-account-list">
              {accounts.map((account) => (
                <div className="settings-account-row" key={account.id}>
                  <span className="avatar avatar-primary" aria-hidden="true">{account.displayName.slice(0, 1) || account.email.slice(0, 1).toUpperCase()}</span>
                  <div className="settings-account-copy">
                    <strong>{account.displayName || account.email}</strong>
                    <span>{account.email}</span>
                    <span>{account.incomingConfigured && account.outgoingConfigured ? '收件与发件已配置' : account.incomingConfigured ? '仅收件' : '仅发件'}</span>
                  </div>
                  {confirmRemoveId === account.id ? (
                    <div className="settings-account-confirm" role="group" aria-label={`确认移除 ${account.email}`}>
                      <span>只移除本地账户，服务器邮件不受影响</span>
                      <div>
                        <button className="text-action" type="button" disabled={removingId === account.id} onClick={() => setConfirmRemoveId(null)}>取消</button>
                        <button className="danger-action" type="button" disabled={removingId === account.id} onClick={() => void removeSelectedAccount(account.id)}>{removingId === account.id ? '正在移除…' : '确认移除'}</button>
                      </div>
                    </div>
                  ) : (
                    <button className="text-action danger-text" type="button" onClick={() => { setAccountError(''); setConfirmRemoveId(account.id); }}>移除</button>
                  )}
                </div>
              ))}
            </div>
          )}
          {accountError && <div className="setting-feedback is-error" role="alert"><Icon name="close" size={16} /><span>{accountError}</span></div>}
        </div>

        <div className="settings-section">
          <div className="settings-section-title">
            <Icon name="sun" size={20} />
            <h2>外观</h2>
          </div>
          <div className="setting-row">
            <div>
              <strong>主题</strong>
              <span>跟随系统，或选择固定主题。</span>
            </div>
            <div className="segmented-control" role="group" aria-label="主题选择">
              {(['system', 'light', 'dark'] as const).map((mode) => (
                <button
                  key={mode}
                  className={themeMode === mode ? 'is-selected' : ''}
                  onClick={() => changeTheme(mode)}
                >
                  {mode === 'system' ? '跟随系统' : mode === 'light' ? '浅色模式' : '深色模式'}
                </button>
              ))}
            </div>
          </div>
        </div>

        <div className="settings-section">
          <div className="settings-section-title">
            <Icon name="shield" size={20} />
            <h2>隐私与安全</h2>
          </div>
          <div className="setting-row">
            <div>
              <strong>安全阅读</strong>
              <span>阻止远程图片、脚本和表单。</span>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={safeReading}
              className={`m3-switch ${safeReading ? 'is-on' : ''}`}
              onClick={changeSafeReading}
              title={safeReading ? '已开启安全阅读' : '已关闭安全阅读'}
            >
              <span className="m3-switch-thumb">
                <Icon name={safeReading ? 'check' : 'close'} size={12} strokeWidth={2.5} />
              </span>
            </button>
          </div>
        </div>

        <div className="settings-section">
          <div className="settings-section-title">
            <Icon name="clock" size={20} />
            <h2>同步与存储</h2>
          </div>
          <div className="setting-row">
            <div>
              <strong>后台同步</strong>
              <span>添加成功后立即同步；之后在应用启动和手动刷新时同步所有已验证账户。</span>
            </div>
            <span className="setting-value">{syncPolicy === 'automatic' ? '自动' : syncPolicy === 'manual' ? '手动' : '已暂停'}</span>
          </div>
          <div className="setting-row">
            <div>
              <strong>本地缓存</strong>
              <span>清除已下载到本地的邮件内容。</span>
            </div>
            <button className="outlined-action" onClick={() => void manageCache()}>
              <Icon name="trash" size={16} />
              <span>清理缓存</span>
            </button>
          </div>
          {cacheStatus && (
            <div className="setting-feedback">
              <Icon name="checkCircle" size={16} />
              <span>{cacheStatus}</span>
            </div>
          )}
        </div>

        <div className="settings-section">
          <div className="settings-section-title">
            <Icon name="monitor" size={20} />
            <h2>关于</h2>
          </div>
          <div className="setting-row">
            <div>
              <strong>Mutsumi Mail</strong>
              <span>版本 0.1.0</span>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
