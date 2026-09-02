import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import type { Message, OutboxItem } from '../types';
import { Icon } from '../lib/icons';
import { clearCache, getSettings, searchMessages, updateSettings } from '../lib/tauri';
import { filterMessages } from '../lib/mail-utils';
import { useUiStore } from '../stores/ui';

export function SearchView({ messages }: { messages: Message[] }) {
  const [query, setQuery] = useState('');
  const localResult = useMemo(() => query.trim() ? filterMessages(messages, query) : messages, [messages, query]);
  const isStructured = query.includes(':');
  const search = useQuery({ queryKey: ['search', query], queryFn: () => searchMessages({ search: query, limit: 100 }), enabled: query.trim().length > 0 && !isStructured, staleTime: 10_000 });
  const result = query.trim() ? (isStructured ? localResult : (search.data ?? localResult)) : messages;
  const selectMessage = useUiStore((state) => state.selectMessage);
  const setNavPage = useUiStore((state) => state.setNavPage);
  useEffect(() => { setNavPage('search'); }, [setNavPage]);
  const navigate = useNavigate();
  return <section className="utility-view"><div className="utility-heading"><div><span className="compose-kicker">本地搜索</span><h1>搜索邮件</h1><p>只搜索已缓存的邮件元数据与正文。</p></div><div className="result-count">{result.length} 个结果</div></div><div className="large-search"><Icon name="search" size={20} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="试试 from:、subject:、is:unread" autoFocus /></div><div className="search-result-list">{result.map((message) => <button key={message.id} className="search-result" onClick={() => { selectMessage(message.id); navigate('/mail'); }}><span className="result-avatar">{message.from.name?.slice(0, 1)}</span><span className="result-copy"><strong>{message.subject}</strong><span>{message.from.name} · {message.preview}</span></span><span className="result-date">{new Date(message.date).toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })}</span></button>)}</div></section>;
}

export function OutboxView({ items }: { items: OutboxItem[] }) {
  const setNavPage = useUiStore((state) => state.setNavPage);
  useEffect(() => { setNavPage('outbox'); }, [setNavPage]);
  return <section className="utility-view"><div className="utility-heading"><div><span className="compose-kicker">可靠操作队列</span><h1>发件箱</h1><p>断网时发送的邮件会先保存到本地，网络恢复后可从这里重试。</p></div><span className="outbox-state"><span className="status-pulse" />{items.length ? `${items.length} 封待处理` : '队列为空'}</span></div>{items.length ? <div className="outbox-list">{items.map((item) => <div className="outbox-item" key={item.id}><div className="outbox-item-icon"><Icon name="sendClock" size={19} /></div><div className="outbox-item-copy"><strong>{item.subject || '无主题邮件'}</strong><span>{item.recipients.join(', ') || '未填写收件人'} · {item.state === 'queued' ? '等待发送' : item.state}</span></div><span className="outbox-item-date">{new Date(item.updatedAt).toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })}</span></div>)}</div> : <div className="outbox-empty"><div className="empty-icon"><Icon name="sendClock" size={28} /></div><h2>没有待处理邮件</h2><p>你发送的草稿会在这里显示状态、重试次数和结果。</p></div>}</section>;
}

export function SettingsView() {
  const { themeMode, setThemeMode } = useUiStore();
  const setNavPage = useUiStore((state) => state.setNavPage);
  const [cacheStatus, setCacheStatus] = useState('');
  useEffect(() => {
    setNavPage('settings');
    void getSettings().then((settings) => setThemeMode(settings.theme)).catch(() => undefined);
  }, [setNavPage, setThemeMode]);
  const changeTheme = (mode: 'system' | 'light' | 'dark') => {
    setThemeMode(mode);
    void updateSettings({ theme: mode }).catch(() => undefined);
  };
  const manageCache = async () => {
    const result = await clearCache();
    setCacheStatus(`已清理 ${result.deletedMessages} 封缓存邮件`);
  };
  return <section className="utility-view settings-view"><div className="utility-heading"><div><span className="compose-kicker">偏好设置</span><h1>设置</h1><p>控制同步、外观与本地隐私。</p></div></div><div className="settings-grid"><div className="settings-section"><div className="settings-section-title"><Icon name="sun" size={19} /><h2>外观</h2></div><div className="setting-row"><div><strong>主题</strong><span>跟随系统，也可以固定深色或浅色。</span></div><div className="segmented-control">{(['system', 'light', 'dark'] as const).map((mode) => <button key={mode} className={themeMode === mode ? 'is-selected' : ''} onClick={() => changeTheme(mode)}>{mode === 'system' ? '系统' : mode === 'light' ? '浅色' : '深色'}</button>)}</div></div></div><div className="settings-section"><div className="settings-section-title"><Icon name="shield" size={19} /><h2>隐私与安全</h2></div><div className="setting-row"><div><strong>安全阅读模式</strong><span>脚本、表单、远程图片默认阻止。</span></div><span className="toggle is-on"><i /></span></div><div className="setting-row"><div><strong>通知内容</strong><span>锁屏时隐藏邮件主题与发件人。</span></div><span className="setting-value">已开启</span></div></div><div className="settings-section"><div className="settings-section-title"><Icon name="clock" size={19} /><h2>同步</h2></div><div className="setting-row"><div><strong>后台同步</strong><span>每 15 分钟检查一次，支持 IDLE 的账户保持连接。</span></div><span className="setting-value">自动</span></div><div className="setting-row"><div><strong>本地缓存</strong><span>正文与附件按照最近使用策略保留。</span></div><button className="text-action" onClick={() => void manageCache()}>管理缓存</button></div>{cacheStatus && <div className="setting-row"><span className="setting-value">{cacheStatus}</span></div>}</div></div></section>;
}
