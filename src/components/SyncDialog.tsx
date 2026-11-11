import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { SyncConfig, SyncProgress, SyncResult } from "../types";

interface Props {
  onClose: () => void;
  onSaved: () => void;
  onSync: (dryRun: boolean) => void;
  busy: boolean;
  progress: SyncProgress | null;
  result: SyncResult | null;
  error: string | null;
}

function formatTime(ts: number): string {
  if (!ts) return "尚未同步";
  return new Date(ts).toLocaleString("zh-CN");
}

export default function SyncDialog({
  onClose,
  onSaved,
  onSync,
  busy,
  progress,
  result,
  error,
}: Props) {
  const [config, setConfig] = useState<SyncConfig | null>(null);
  const [dataDir, setDataDir] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [cfg, dir] = await Promise.all([
        api.syncGetConfig(),
        api.appDataDir(),
      ]);
      setConfig(cfg);
      setDataDir(dir);
    } catch (e) {
      setLocalError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const patch = (p: Partial<SyncConfig>) =>
    setConfig((c) => (c ? { ...c, ...p } : c));

  const save = async (): Promise<boolean> => {
    if (!config) return false;
    try {
      await api.syncSetConfig(config);
      setNotice("配置已保存");
      onSaved();
      return true;
    } catch (e) {
      setLocalError(String(e));
      return false;
    }
  };

  const test = async () => {
    if (!config) return;
    setTesting(true);
    setNotice(null);
    setLocalError(null);
    try {
      // Test the values currently in the form, even before saving.
      setNotice(await api.syncTest(config));
    } catch (e) {
      setLocalError(String(e));
    } finally {
      setTesting(false);
    }
  };

  const sync = async (dryRun: boolean) => {
    setNotice(null);
    setLocalError(null);
    if (!(await save())) return;
    onSync(dryRun);
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal sync-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="sync-head">
          <h2>同步设置 · WebDAV</h2>
          <button className="icon-btn" onClick={onClose} title="关闭">
            ×
          </button>
        </div>

        <p className="sync-hint">
          与官方 Joplin 客户端使用同一个 WebDAV 目录。本应用只从这里读取数据：
          全部笔记、笔记本、标签与附件都由同步下载，不读取本机 Joplin 的任何文件。
        </p>

        {config ? (
          <div className="sync-form">
            <label>
              <span>WebDAV 地址</span>
              <input
                value={config.url}
                placeholder="https://example.com/remote.php/dav/files/user/Joplin/"
                onChange={(e) => patch({ url: e.target.value })}
              />
            </label>
            <div className="sync-row">
              <label>
                <span>用户名</span>
                <input
                  value={config.username}
                  onChange={(e) => patch({ username: e.target.value })}
                />
              </label>
              <label>
                <span>密码</span>
                <input
                  type="password"
                  value={config.password}
                  placeholder="WebDAV 密码"
                  onChange={(e) => patch({ password: e.target.value })}
                />
              </label>
            </div>
            <div className="sync-checks">
              <label className="check">
                <input
                  type="checkbox"
                  checked={config.autoSync}
                  onChange={(e) => patch({ autoSync: e.target.checked })}
                />
                <span>自动同步</span>
              </label>
              <label className="check inline-number">
                <span>间隔</span>
                <input
                  type="number"
                  min={1}
                  max={720}
                  value={config.intervalMinutes}
                  onChange={(e) =>
                    patch({ intervalMinutes: Number(e.target.value) || 1 })
                  }
                />
                <span>分钟</span>
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={config.allowInvalidCerts}
                  onChange={(e) =>
                    patch({ allowInvalidCerts: e.target.checked })
                  }
                />
                <span>允许自签名证书</span>
              </label>
            </div>
            <div className="sync-meta">
              上次同步：{formatTime(config.lastSyncAt)}
              {config.lastMessage ? ` · ${config.lastMessage}` : ""}
            </div>
          </div>
        ) : (
          <div className="sync-loading">正在读取同步配置…</div>
        )}

        {(notice || localError || error) && (
          <div className={`sync-notice ${localError || error ? "bad" : ""}`}>
            {localError || error || notice}
          </div>
        )}

        {busy && (
          <div className="sync-progress">
            <div className="sync-progress-label">
              {progress?.message ?? "正在同步…"}
            </div>
            {progress && progress.total > 0 && (
              <div className="sync-bar">
                <div
                  style={{
                    width: `${Math.min(
                      100,
                      (progress.current / progress.total) * 100,
                    )}%`,
                  }}
                />
              </div>
            )}
          </div>
        )}

        {result && !busy && (
          <div className="sync-result">
            <strong>{result.dryRun ? "预演结果" : "同步结果"}</strong>
            <div>
              下载 {result.downloaded} · 上传 {result.uploaded} · 跳过{" "}
              {result.skipped} · 本地删除 {result.deletedLocal} · 远端删除{" "}
              {result.deletedRemote}
            </div>
            <div>
              远端条目 {result.remoteItems} · 本地条目 {result.localItems} ·
              附件 ↓{result.resourcesDown} ↑{result.resourcesUp}
            </div>
            {result.unsupported > 0 && (
              <div className="sync-ignored">
                忽略 {result.unsupported} 个本应用不支持的远端条目（Joplin 修订历史等）。
                这些文件在服务器上保持原样，不会被下载、修改或删除。
              </div>
            )}
            {result.conflicts > 0 && (
              <div className="sync-conflicts">
                <div>
                  ⚠ {result.conflicts} 篇笔记在两端都被修改过：较新的一份保留原笔记，
                  另一份已存为「…（冲突副本 时间）」并同步到服务器，两份内容都没有丢失。
                </div>
                {result.conflictNotes.slice(0, 5).map((n, i) => (
                  <div key={i}>· {n}</div>
                ))}
                {result.conflictNotes.length > 5 && (
                  <div>… 另有 {result.conflictNotes.length - 5} 条</div>
                )}
              </div>
            )}
            {result.message && <div className="sync-message">{result.message}</div>}
            {result.errors.length > 0 && (
              <div className="sync-errors">
                {result.errors.slice(0, 5).map((e, i) => (
                  <div key={i}>⚠ {e}</div>
                ))}
                {result.errors.length > 5 && (
                  <div>… 另有 {result.errors.length - 5} 条</div>
                )}
              </div>
            )}
          </div>
        )}

        <div className="sync-actions">
          <button
            className="ghost-btn"
            disabled={testing || !config}
            onClick={() => void test()}
          >
            {testing ? "测试中…" : "测试连接"}
          </button>
          <button className="ghost-btn" onClick={() => void save()}>
            保存
          </button>
          <span className="spacer" />
          <button
            className="ghost-btn"
            disabled={busy}
            onClick={() => void sync(true)}
          >
            预演（不写入）
          </button>
          <button
            className="primary-btn"
            disabled={busy}
            onClick={() => void sync(false)}
          >
            {busy ? "同步中…" : "立即同步"}
          </button>
        </div>

        <p className="sync-footnote">
          密码以明文保存在本应用的 sync.json（仅当前用户目录可读）。首次建议先「预演」查看将上传/下载的数量，再执行同步。
        </p>

        <div className="sync-datadir">
          <span>应用数据目录</span>
          <code>{dataDir ?? "读取中…"}</code>
          <small>配置、数据库、附件与日志均存放于此；可用 JOPLIN_RS_DATA_DIR 覆盖。</small>
        </div>
      </div>
    </div>
  );
}
