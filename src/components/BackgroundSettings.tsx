import { useEffect, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { listen } from '@tauri-apps/api/event';
import { getBackgroundStatus, openBackgroundSettings, setLaunchAtLogin } from '../lib/background';
import { appErrorMessage, isTauriRuntime, updateSettings } from '../lib/tauri';
import { Icon } from '../lib/icons';

export function BackgroundSettings() {
  const client = useQueryClient();
  const status = useQuery({ queryKey: ['background-status'], queryFn: getBackgroundStatus, enabled: isTauriRuntime });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    if (!isTauriRuntime) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const refresh = () => { void client.invalidateQueries({ queryKey: ['background-status'] }); };
    window.addEventListener('focus', refresh);
    void listen('background-state-changed', refresh).then((dispose) => {
      if (disposed) dispose(); else unlisten = dispose;
    }).catch(() => {});
    return () => { disposed = true; unlisten?.(); window.removeEventListener('focus', refresh); };
  }, [client]);
  if (!isTauriRuntime) return null;
  const data = status.data;
  const change = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true); setError('');
    try { await action(); await status.refetch(); }
    catch (reason) { setError(appErrorMessage(reason)); }
    finally { setBusy(false); }
  };
  return (
    <div className="settings-section background-settings">
      <div className="settings-section-title"><Icon name="inbox" size={20} /><h2>后台收信</h2></div>
      <div className="setting-row">
        <div><strong>离开窗口后继续收信</strong><span>自动收件账户保持推送连接；没有新邮件时减少同步工作。</span></div>
        <button type="button" className="outlined-action" role="switch" aria-label="离开窗口后继续收信"
          aria-checked={data?.backgroundMail ?? false} disabled={busy || !data}
          onClick={() => void change(() => updateSettings({ backgroundMail: !data?.backgroundMail }))}>
          {data?.backgroundMail ? '已开启' : '已关闭'}
        </button>
      </div>
      {data?.autostartSupported && <div className="setting-row">
        <div><strong>登录时启动</strong><span>登录电脑后在托盘中启动，点击托盘图标可打开邮箱。</span></div>
        <button type="button" className="outlined-action" role="switch" aria-label="登录时启动"
          aria-checked={Boolean(data.launchAtLogin)} disabled={busy}
          onClick={() => void change(() => setLaunchAtLogin(!data.launchAtLogin))}>
          {data.launchAtLogin ? '已开启' : '已关闭'}
        </button>
      </div>}
      {data?.platform === 'android' && <>
        <div className="setting-row"><div><strong>后台运行状态</strong><span>
          {data.quotaExpired ? '本轮后台运行时间已用完，暂由系统安排检查；打开应用后恢复实时收信。'
            : data.serviceRunning ? (data.online ? '后台收信服务正在运行。' : '等待网络恢复后自动继续收信。')
              : '有自动收件账户时启动后台服务，系统还会安排定期检查。'}
        </span></div></div>
        <div className="setting-row"><div><strong>电池优化</strong><span>
          {data.batteryUnrestricted ? '已允许不受电池优化限制的后台运行。'
            : '系统省电可能延迟锁屏后的收信，可在系统设置中允许邮箱后台运行。'}
        </span></div><button type="button" className="outlined-action" disabled={busy}
          onClick={() => void change(openBackgroundSettings)}>管理后台权限</button></div>
      </>}
      {(error || status.error) && <div className="setting-feedback" role="alert">{error || appErrorMessage(status.error)}</div>}
    </div>
  );
}
