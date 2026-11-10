import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import Sidebar from "./components/Sidebar";
import NoteList from "./components/NoteList";
import ContextMenu from "./components/ContextMenu";
import PrintSheet, { type PrintJob } from "./components/PrintSheet";
import SyncDialog from "./components/SyncDialog";
// The two editors are the heaviest dependencies. Textus-style startup budget:
// each is fetched only when its editing view is actually opened, and they are
// never mounted at the same time.
const MarkdownEditor = lazy(() => import("./components/MarkdownEditor"));
const WysiwygEditor = lazy(() => import("./components/WysiwygEditor"));
import type {
  Bootstrap,
  Folder,
  Note,
  NoteSummary,
  PdfHeading,
  RenderedNote,
  Selection,
  Stats,
  SyncProgress,
  SyncResult,
  SyncStatus,
  Tag,
  ViewMode,
} from "./types";

type SaveStatus = "saved" | "dirty" | "saving";

/** Relative label for the top-bar sync stamp: "刚刚 / 12 分钟前 / 3 小时前 /
    11月9日 14:20". `lastSyncAt` is epoch milliseconds written by the Rust
    sync engine, and 0 means "never synced". */
function formatSyncStamp(ts: number): string {
  if (!ts) return "尚未同步";
  const diff = Date.now() - ts;
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const date = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getMonth() + 1}月${date.getDate()}日 ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Last-modified stamp for the editor header. Short and unambiguous:
    "今天 21:05" / "11月9日 21:05" / "2025年11月9日". */
function formatUpdated(ts: number): string {
  if (!ts) return "—";
  const date = new Date(ts);
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  const hm = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (date.toDateString() === now.toDateString()) return `今天 ${hm}`;
  if (date.getFullYear() === now.getFullYear()) {
    return `${date.getMonth() + 1}月${date.getDate()}日 ${hm}`;
  }
  return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
}

/** One-line milestone for the sidebar ticker. The SQLite-level phases (the
    "apply" plan, connection chatter) only make sense inside the dialog, so they
    are dropped here. */
function tickerLine(progress: SyncProgress): string | null {
  switch (progress.phase) {
    case "apply":
    case "connect":
      return null;
    case "done":
      return progress.message.startsWith("预演") ? "预演已完成" : "已完成";
    default: {
      const text = progress.message.trim();
      return text || null;
    }
  }
}

export default function App() {
  const [bootstrap, setBootstrap] = useState<Bootstrap | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [stats, setStats] = useState<Stats | null>(null);
  const [notes, setNotes] = useState<NoteSummary[]>([]);
  const [selection, setSelection] = useState<Selection>({ kind: "all" });
  const [activeNote, setActiveNote] = useState<Note | null>(null);
  const [rendered, setRendered] = useState<RenderedNote | null>(null);
  // 打字机（WYSIWYG）是默认视图；源码是开关的「打开」状态。
  const [viewMode, setViewMode] = useState<ViewMode>("wysiwyg");
  const [searchInput, setSearchInput] = useState("");
  const [status, setStatus] = useState<SaveStatus>("saved");
  const [error, setError] = useState<string | null>(null);
  const [showAbout, setShowAbout] = useState(false);
  const [showSync, setShowSync] = useState(false);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [syncBusy, setSyncBusy] = useState(false);
  const [syncProgress, setSyncProgress] = useState<SyncProgress | null>(null);
  const [syncResult, setSyncResult] = useState<SyncResult | null>(null);
  const [syncError, setSyncError] = useState<string | null>(null);
  // Rolling milestones shown next to the sidebar sync button, e.g.
  // "已获取条目：1/1。已新建本地条目：1。已完成".
  const [syncFeed, setSyncFeed] = useState<string[]>([]);
  const lastSyncPhase = useRef("");
  // Folder creation used to call window.prompt(), which WebView2 renders as a
  // native dialog titled "tauri.localhost 显示". Keep it inside the app instead.
  const [folderDraft, setFolderDraft] = useState<{
    parentId: string;
    title: string;
  } | null>(null);

  // Right-click menu of the note list, plus the two things it can trigger.
  const [menu, setMenu] = useState<{
    note: NoteSummary;
    x: number;
    y: number;
  } | null>(null);
  const [pendingDelete, setPendingDelete] = useState<NoteSummary | null>(null);
  const [printJob, setPrintJob] = useState<PrintJob | null>(null);
  const [pdfBusy, setPdfBusy] = useState(false);
  // Non-error feedback ("已导出 PDF …"); the `error` toast stays reserved for
  // things that actually went wrong.
  const [notice, setNotice] = useState<string | null>(null);
  const printReady = useRef<(() => void) | null>(null);
  // Filled in by the print sheet: the note's headings, in document order, for
  // the PDF's bookmark tree.
  const printHeadings = useRef<PdfHeading[]>([]);

  // Re-render once a minute so "12 分钟前" stays truthful without polling the
  // backend. Only runs while a timestamp actually exists.
  const [, setClockTick] = useState(0);
  useEffect(() => {
    if (!syncStatus?.lastSyncAt) return;
    const timer = window.setInterval(() => setClockTick((n) => n + 1), 60_000);
    return () => window.clearInterval(timer);
  }, [syncStatus?.lastSyncAt]);

  const searchRef = useRef<HTMLInputElement>(null);
  const activeRef = useRef<Note | null>(null);
  const dirtyRef = useRef(false);
  const saveTimer = useRef<number | null>(null);
  const renderTimer = useRef<number | null>(null);
  const noticeTimer = useRef<number | null>(null);

  const refreshNotes = useCallback(async () => {
    // Search results are managed by the search effect; never clobber them.
    if (selection.kind === "search") return;
    try {
      let list: NoteSummary[];
      if (selection.kind === "folder") {
        list = await api.listNotes({ folderId: selection.id });
      } else if (selection.kind === "tag") {
        list = await api.listNotes({ tagId: selection.id });
      } else {
        list = await api.listNotes({});
      }
      setNotes(list);
    } catch (e) {
      setError(String(e));
    }
  }, [selection]);

  const loadShell = useCallback(async () => {
    const [nextFolders, nextTags, nextStats] = await Promise.all([
      api.listFolders(),
      api.listTags(),
      api.getStats(),
    ]);
    setFolders(nextFolders);
    setTags(nextTags);
    setStats(nextStats);
  }, []);

  useEffect(() => {
    void (async () => {
      try {
        const result = await api.bootstrap();
        setBootstrap(result);
        if (result.ok) {
          await loadShell();
          await refreshNotes();
        }
        void api.syncStatus().then(setSyncStatus).catch(() => {});
      } catch (e) {
        setError(String(e));
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (bootstrap?.ok && selection.kind !== "search") void refreshNotes();
  }, [selection, bootstrap?.ok, refreshNotes]);

  useEffect(() => {
    if (!bootstrap?.ok) return;
    const query = searchInput.trim();
    const timer = window.setTimeout(() => {
      if (!query) {
        if (selection.kind === "search") setSelection({ kind: "all" });
        else void refreshNotes();
        return;
      }
      api
        .searchNotes(query)
        .then((hits) => {
          setNotes(hits);
          setSelection({ kind: "search", query });
        })
        .catch((e) => setError(String(e)));
    }, 220);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchInput, bootstrap?.ok]);

  // Writes are serialised through this chain. Without it, clicking note B and
  // straight back to A let the second `openNote` read the database while A's
  // write was still in flight: the editor got the pre-edit body, `dirty` was
  // already false, and the next keystroke saved that stale body back — the edit
  // silently vanished. `flushSave` now returns the pending write when there is
  // nothing new to save, so `openNote` only reads after it landed.
  const saveChain = useRef<Promise<void>>(Promise.resolve());

  const flushSave = useCallback(
    async (explicit?: Note) => {
      const note = explicit ?? activeRef.current;
      if (!note || !dirtyRef.current) return saveChain.current;
      // Snapshot now: `activeRef` may move on to another note before the write
      // runs, and the debounce timer passes its own (newer) snapshot in.
      const snapshot = { id: note.id, title: note.title, body: note.body };
      dirtyRef.current = false;
      setStatus("saving");
      const write = saveChain.current.then(async () => {
        try {
          await api.saveNote(snapshot.id, snapshot.title, snapshot.body);
          // The backend bumped updated_time; mirror it so the header stamp
          // reflects the save without waiting for a re-open.
          if (activeRef.current?.id === snapshot.id) {
            activeRef.current = { ...activeRef.current, updatedTime: Date.now() };
            setActiveNote(activeRef.current);
          }
          setStatus("saved");
          await refreshNotes();
        } catch (e) {
          dirtyRef.current = true;
          setStatus("dirty");
          setError(String(e));
        }
      });
      saveChain.current = write;
      await write;
    },
    [refreshNotes],
  );

  // A note switch is two async round-trips; the second click must win even if
  // the first one resolves last.
  const openTicket = useRef(0);

  const openNote = useCallback(
    async (id: string) => {
      const ticket = ++openTicket.current;
      // Never read before the previous note's edit has been written.
      await flushSave();
      try {
        const [note, html] = await Promise.all([
          api.getNote(id),
          api.renderNote(id),
        ]);
        if (ticket !== openTicket.current) return;
        activeRef.current = note;
        setActiveNote(note);
        setRendered(html);
        dirtyRef.current = false;
        setStatus("saved");
        setError(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [flushSave],
  );

  const onDraftChange = (patch: Partial<Pick<Note, "title" | "body">>) => {
    const current = activeRef.current;
    if (!current) return;
    const next = { ...current, ...patch };
    activeRef.current = next;
    setActiveNote(next);
    dirtyRef.current = true;
    setStatus("dirty");

    if (renderTimer.current) window.clearTimeout(renderTimer.current);
    if (patch.body !== undefined) {
      renderTimer.current = window.setTimeout(() => {
        void api.renderMarkdown(next.body).then(setRendered).catch(() => {});
      }, 160);
    }
    if (saveTimer.current) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => void flushSave(next), 900);
  };

  /** Drag-and-drop move: a note card dropped on a notebook in the sidebar. */
  const moveNoteToFolder = useCallback(
    async (noteId: string, folderId: string) => {
      try {
        const moved = await api.moveNote(noteId, folderId);
        if (activeRef.current?.id === noteId) {
          activeRef.current = {
            ...activeRef.current,
            parentId: moved.parentId,
            updatedTime: moved.updatedTime,
          };
          setActiveNote(activeRef.current);
        }
        await loadShell();
        if (selection.kind !== "search") await refreshNotes();
      } catch (e) {
        setError(String(e));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [loadShell, refreshNotes, selection.kind],
  );

  /** Context menu → 删除：soft-delete, same tombstone flow as Joplin itself. */
  const deleteNote = useCallback(
    async (target: NoteSummary) => {
      // Neighbours are captured before the list is reloaded: after the delete
      // the editor should land on the next note instead of going blank.
      const before = notes;
      const at = before.findIndex((n) => n.id === target.id);
      const wasActive = activeRef.current?.id === target.id;
      try {
        await api.deleteNote(target.id);
        if (wasActive) {
          if (saveTimer.current) window.clearTimeout(saveTimer.current);
          dirtyRef.current = false;
          activeRef.current = null;
          setActiveNote(null);
          setRendered(null);
          setStatus("saved");
        }
        await loadShell();
        if (selection.kind === "search" && searchInput.trim()) {
          setNotes(await api.searchNotes(searchInput.trim()));
        } else {
          await refreshNotes();
        }
        if (wasActive) {
          const neighbour = before[at + 1] ?? before[at - 1];
          if (neighbour && neighbour.id !== target.id) await openNote(neighbour.id);
        }
        if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
        setNotice(`已删除「${target.title || "无标题笔记"}」`);
        noticeTimer.current = window.setTimeout(() => setNotice(null), 4000);
      } catch (e) {
        setError(String(e));
      }
    },
    [notes, selection, searchInput, loadShell, refreshNotes, openNote],
  );

  /** Context menu → 导出为 PDF.
   *
   * WebView2 prints the DOM, so the order matters: put the note on a print-only
   * sheet, wait for fonts and attachments to settle, then let Rust write the
   * file. `pdfBusy` keeps the rest of the UI out of the way meanwhile.
   */
  const exportPdf = useCallback(
    async (target: NoteSummary) => {
      if (pdfBusy) return;
      setPdfBusy(true);
      printHeadings.current = [];
      try {
        // The PDF is built from the database copy, so pending edits have to land
        // first — otherwise the export silently omits the last keystrokes.
        await flushSave();
        const note = await api.getNote(target.id);
        const name =
          (note.title || "无标题笔记").replace(/[\\/:*?"<>|]/g, "_").trim() ||
          "无标题笔记";
        const path = await save({
          title: "导出为 PDF",
          defaultPath: `${name}.pdf`,
          filters: [{ name: "PDF 文档", extensions: ["pdf"] }],
        });
        if (!path) return;
        const html = await api.renderNote(note.id);
        const ready = new Promise<void>((resolve) => {
          printReady.current = resolve;
        });
        setPrintJob({
          title: note.title,
          folder: folders.find((f) => f.id === note.parentId)?.title ?? "",
          updatedLabel: formatUpdated(note.updatedTime),
          tags: note.tags,
          html: html.html,
        });
        // The sheet signals readiness itself; the race keeps a broken
        // attachment from stalling the export indefinitely.
        await Promise.race([
          ready,
          new Promise((resolve) => window.setTimeout(resolve, 4000)),
        ]);
        await api.exportNotePdf(path, printHeadings.current);
        if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
        setNotice(`已导出 PDF：${path}`);
        noticeTimer.current = window.setTimeout(() => setNotice(null), 6000);
      } catch (e) {
        setError(String(e));
      } finally {
        printReady.current = null;
        setPrintJob(null);
        setPdfBusy(false);
      }
    },
    [pdfBusy, folders, flushSave],
  );

  const handlePrintReady = useCallback((headings: PdfHeading[]) => {
    printHeadings.current = headings;
    printReady.current?.();
  }, []);

  // While a PDF is being written the window shows only the print sheet; the
  // class is what swaps `.app-shell` out (see `.print-sheet` in styles.css).
  useEffect(() => {
    document.body.classList.toggle("printing", printJob !== null);
    return () => document.body.classList.remove("printing");
  }, [printJob]);

  const newNote = useCallback(async () => {
    if (!bootstrap?.ok) return;
    await flushSave();
    let folderId = selection.kind === "folder" ? selection.id : "";
    if (!folderId) folderId = folders[0]?.id ?? "";
    if (!folderId) {
      setError("请先创建一个笔记本");
      return;
    }
    try {
      const note = await api.createNote(folderId, "无标题笔记");
      if (selection.kind === "search") {
        setSearchInput("");
        setSelection({ kind: "all" });
        setNotes(await api.listNotes({}));
      } else {
        await refreshNotes();
      }
      await loadShell();
      await openNote(note.id);
    } catch (e) {
      setError(String(e));
    }
  }, [bootstrap?.ok, selection, folders, flushSave, loadShell, refreshNotes, openNote]);

  const newFolder = (parentId: string) => {
    setFolderDraft({ parentId, title: "" });
  };

  const createFolder = async () => {
    const draft = folderDraft;
    const title = draft?.title.trim();
    if (!draft || !title) return;
    setFolderDraft(null);
    try {
      await api.createFolder(title, draft.parentId);
      await loadShell();
    } catch (e) {
      setError(String(e));
    }
  };

  const runSync = useCallback(
    async (dryRun: boolean) => {
      if (syncBusy) return;
      setSyncBusy(true);
      setSyncResult(null);
      setSyncError(null);
      setSyncProgress(null);
      setSyncFeed([]);
      lastSyncPhase.current = "";
      try {
        const result = dryRun ? await api.syncPreview() : await api.syncRun();
        setSyncResult(result);
        if (!dryRun) {
          await loadShell();
          await refreshNotes();
          setSyncStatus(await api.syncStatus());
        }
      } catch (e) {
        setSyncError(String(e));
        setSyncFeed((prev) => [...prev, "同步失败"].slice(-3));
      } finally {
        setSyncBusy(false);
      }
    },
    [syncBusy, loadShell, refreshNotes],
  );

  /** Sidebar sync button: runs right away in the background. Without saved
      WebDAV settings there is nothing to sync, so open the settings instead. */
  const syncFromSidebar = useCallback(() => {
    if (syncBusy) return;
    if (!syncStatus?.configured) {
      setShowSync(true);
      return;
    }
    void runSync(false);
  }, [syncBusy, syncStatus?.configured, runSync]);

  // Native menu events. Subscribe once and route through a ref so the handler
  // always sees fresh state without re-subscribing on every render.
  const menuHandlerRef = useRef<(id: string) => void>(() => {});
  menuHandlerRef.current = (id: string) => {
    switch (id) {
      case "file_new_note":
        void newNote();
        break;
      case "file_quit":
        void api.appQuit();
        break;
      case "view_source":
        setViewMode("source");
        break;
      case "view_wysiwyg":
        setViewMode("wysiwyg");
        break;
      case "tools_sync_now":
        setShowSync(true);
        void runSync(false);
        break;
      case "tools_sync_preview":
        setShowSync(true);
        void runSync(true);
        break;
      case "tools_sync_settings":
        setShowSync(true);
        break;
      case "help_about":
        setShowAbout(true);
        break;
    }
  };

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void listen<string>("menu://action", (event) =>
      menuHandlerRef.current(event.payload),
    ).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    let progress: (() => void) | null = null;
    let done: (() => void) | null = null;
    void listen<SyncProgress>("sync://progress", (event) => {
      setSyncProgress(event.payload);
      const line = tickerLine(event.payload);
      if (!line) return;
      // Repeated updates inside one phase replace that line instead of piling
      // up; the ticker keeps the last three milestones of this run.
      const samePhase = lastSyncPhase.current === event.payload.phase;
      lastSyncPhase.current = event.payload.phase;
      setSyncFeed((prev) =>
        (samePhase ? [...prev.slice(0, -1), line] : [...prev, line]).slice(-3),
      );
    }).then((fn) => {
      progress = fn;
    });
    void listen<SyncResult>("sync://done", (event) =>
      setSyncResult(event.payload),
    ).then((fn) => {
      done = fn;
    });
    return () => {
      progress?.();
      done?.();
    };
  }, []);

  useEffect(() => {
    if (!syncStatus?.autoSync || !syncStatus.configured) return;
    const ms = Math.max(1, syncStatus.intervalMinutes) * 60_000;
    const id = window.setInterval(() => {
      if (!syncBusy) void runSync(false);
    }, ms);
    return () => window.clearInterval(id);
  }, [syncStatus, syncBusy, runSync]);

  // Keyboard shortcuts. Re-bound each render so handlers always see fresh state.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      // DevTools before the modifier check: F12 carries no modifier, and
      // Ctrl+Shift+I mirrors the browser (and Electron's default).
      if (
        event.key === "F12" ||
        (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === "i")
      ) {
        event.preventDefault();
        void api.openDevtools();
        return;
      }
      const mod = event.ctrlKey || event.metaKey;
      if (!mod) return;
      const key = event.key.toLowerCase();
      if (key === "s") {
        // Same action as the sidebar's sync button, but write the open note
        // first: a sync pushes the local database, so unsaved edits would be
        // left behind (or worse, conflict with the pushed copy).
        event.preventDefault();
        void flushSave().then(() => syncFromSidebar());
      } else if (key === "n") {
        event.preventDefault();
        void newNote();
      } else if (key === "f") {
        event.preventDefault();
        searchRef.current?.focus();
      } else if (key === "e") {
        event.preventDefault();
        setViewMode((mode) => (mode === "source" ? "wysiwyg" : "source"));
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  });

  useEffect(() => {
    const handler = () => void flushSave();
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [flushSave]);

  // The note list header no longer repeats the current section name, so the
  // editor header carries the note's own provenance instead.
  const activeFolder = activeNote
    ? folders.find((f) => f.id === activeNote.parentId)
    : undefined;

  if (!bootstrap) {
    return <div className="loading-screen">正在读取 Joplin 数据库…</div>;
  }

  if (!bootstrap.ok) {
    return (
      <div className="setup-screen">
        <div className="setup-card">
          <div className="setup-logo"><img src="/app-icon.svg" alt="Joplin RS" /></div>
          <h1>Joplin RS</h1>
          <p>无法打开本地笔记库。</p>
          <code>{bootstrap.error ?? "未知错误"}</code>
          <p className="setup-hint">
            数据目录：
            <br />
            <code>{bootstrap.dataDir}</code>
          </p>
          <p className="setup-note">
            本应用只从 WebDAV 读取数据，本地库为空时会自动重建。若该目录被占用或不可写，请检查权限后重试。
          </p>
          <div className="setup-actions">
            <button
              className="primary-btn"
              onClick={() => window.location.reload()}
            >
              重新加载
            </button>
            <button className="ghost-btn" onClick={() => void api.appQuit()}>
              退出
            </button>
          </div>
        </div>
      </div>
    );
  }

  // A brand-new (or manually cleared) data directory is empty by design:
  // nothing is imported from any local Joplin profile, the library is filled
  // by the first WebDAV sync.
  if (stats && stats.notes === 0 && stats.folders === 0) {
    return (
      <div className="setup-screen">
        <div className="setup-card">
          <div className="setup-logo"><img src="/app-icon.svg" alt="Joplin RS" /></div>
          <h1>Joplin RS</h1>
          <p>本地笔记库是空的。本应用只从 WebDAV 读取数据，配置同步后会自动下载全部笔记与附件。</p>
          <ol className="setup-steps">
            <li>填写 WebDAV 地址、用户名与密码</li>
            <li>「测试连接」确认可访问</li>
            <li>「立即同步」把云端笔记拉到本地</li>
          </ol>
          <div className="setup-actions">
            <button className="primary-btn" onClick={() => setShowSync(true)}>
              配置 WebDAV 同步
            </button>
          </div>
          <p className="setup-note">
            数据目录：<code>{bootstrap.dataDir}</code>
          </p>
        </div>

        {showSync && (
          <SyncDialog
            onClose={() => setShowSync(false)}
            onSaved={() => {
              void api.syncStatus().then(setSyncStatus).catch(() => {});
            }}
            onSync={(dryRun) => void runSync(dryRun)}
            busy={syncBusy}
            progress={syncProgress}
            result={syncResult}
            error={syncError}
          />
        )}

        {error && (
          <div className="toast" onClick={() => setError(null)}>
            <span>⚠ {error}</span>
            <button>×</button>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="app-shell">
      <div className="main-row">
        <Sidebar
          folders={folders}
          tags={tags}
          selection={selection}
          totalNotes={stats?.notes ?? 0}
          syncConfigured={Boolean(syncStatus?.configured)}
          syncBusy={syncBusy}
          syncTitle={
            syncStatus?.lastSyncAt
              ? `上次同步：${new Date(syncStatus.lastSyncAt).toLocaleString("zh-CN")}`
              : "还没有成功同步过"
          }
          onOpenSync={() => setShowSync(true)}
          onSyncNow={syncFromSidebar}
          syncFeed={syncFeed.join("。")}
          onSelect={(next) => {
            if (next.kind !== "search") setSearchInput("");
            setSelection(next);
          }}
          onNewFolder={(id) => void newFolder(id)}
          onMoveNote={(noteId, folderId) =>
            void moveNoteToFolder(noteId, folderId)
          }
        />

        <NoteList
          notes={notes}
          activeId={activeNote?.id ?? null}
          contextId={menu?.note.id ?? null}
          searchValue={searchInput}
          searchRef={searchRef}
          onSearchChange={setSearchInput}
          onSelect={(id) => void openNote(id)}
          onNew={() => void newNote()}
          onCardContext={(note, x, y) => setMenu({ note, x, y })}
        />

        <main className="workspace">
          <div className="editor-header">
            <div className="title-row">
              <input
                className="title-input"
                value={activeNote?.title ?? ""}
                placeholder="无标题笔记"
                disabled={!activeNote}
                onChange={(e) => onDraftChange({ title: e.target.value })}
              />
              {activeNote && (
                <div
                  className="title-meta"
                  title={
                    activeNote.updatedTime
                      ? `最后更新：${new Date(
                          activeNote.updatedTime,
                        ).toLocaleString("zh-CN")}`
                      : undefined
                  }
                >
                  <span className="title-folder">
                    <span className="title-folder-icon" aria-hidden="true">
                      📖
                    </span>
                    {activeFolder?.title || "未归入笔记本"}
                  </span>
                  <span className="title-meta-sep">·</span>
                  <span>更新于 {formatUpdated(activeNote.updatedTime)}</span>
                </div>
              )}
              {/* The view switch rides on the title row: a whole extra row for
                  one 36px pill wasted too much vertical space. */}
              <div className="mode-toggle">
                <button
                  type="button"
                  role="switch"
                  aria-checked={viewMode === "source"}
                  aria-label="源码模式"
                  title="Ctrl+E 切换（关闭时为打字机模式）"
                  className="switch"
                  onClick={() =>
                    setViewMode(viewMode === "source" ? "wysiwyg" : "source")
                  }
                >
                  <span className="knob" />
                </button>
                <span className="mode-toggle-label">源码模式</span>
              </div>
            </div>
            {activeNote && activeNote.tags.length > 0 && (
              <div className="meta-line">
                <span className="tag-line">
                  {activeNote.tags.map((t) => `#${t}`).join("  ")}
                </span>
              </div>
            )}
          </div>

          <div className={`editor-body mode-${viewMode}`}>
            {activeNote && viewMode === "source" && (
              <div className="pane source-pane">
                <Suspense
                  fallback={<div className="pane-loading">正在加载编辑器…</div>}
                >
                  <MarkdownEditor
                    key={activeNote.id}
                    value={activeNote.body}
                    onChange={(body) => onDraftChange({ body })}
                  />
                </Suspense>
              </div>
            )}
            {activeNote && viewMode === "wysiwyg" && (
              <div className="pane wysiwyg-pane">
                <Suspense
                  fallback={<div className="pane-loading">正在加载编辑器…</div>}
                >
                  <WysiwygEditor
                    key={activeNote.id}
                    value={activeNote.body}
                    onChange={(body) => onDraftChange({ body })}
                    onOpenNote={(id) => void openNote(id)}
                    onFallback={() => setViewMode("source")}
                  />
                </Suspense>
              </div>
            )}
          </div>

          <footer className="status-bar">
            <span className={`save-dot ${status}`} />
            <span>
              {status === "saving"
                ? "保存中…"
                : status === "dirty"
                  ? "未保存"
                  : "已保存"}
            </span>
            <span className="status-sep">·</span>
            <span>{rendered?.wordCount ?? 0} 词</span>
            <span className="status-sep">·</span>
            <span>{rendered?.charCount ?? 0} 字符</span>
            <span className="spacer" />
            <span
              className="status-sync"
              title={
                syncStatus?.lastSyncAt
                  ? `上次同步：${new Date(syncStatus.lastSyncAt).toLocaleString("zh-CN")}`
                  : "还没有成功同步过"
              }
            >
              {syncBusy
                ? "正在同步…"
                : syncStatus?.lastSyncAt
                  ? `上次同步 ${formatSyncStamp(syncStatus.lastSyncAt)}`
                  : "尚未同步"}
            </span>
            <span className="status-sep">·</span>
            <span className="status-version" title="应用版本">
              v{bootstrap?.version ?? "—"}
            </span>
          </footer>
        </main>
      </div>

      {showSync && (
        <SyncDialog
          onClose={() => setShowSync(false)}
          onSaved={() => {
            void api.syncStatus().then(setSyncStatus).catch(() => {});
          }}
          onSync={(dryRun) => void runSync(dryRun)}
          busy={syncBusy}
          progress={syncProgress}
          result={syncResult}
          error={syncError}
        />
      )}

      {error && (
        <div className="toast" onClick={() => setError(null)}>
          <span>⚠ {error}</span>
          <button>×</button>
        </div>
      )}

      {notice && (
        <div className="toast toast-ok" onClick={() => setNotice(null)}>
          <span>✓ {notice}</span>
          <button>×</button>
        </div>
      )}

      {showAbout && (
        <div className="modal-backdrop" onClick={() => setShowAbout(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2>关于 Joplin RS</h2>
            <p>
              用 Rust + Tauri 2 重写的 Joplin 客户端。数据存储、Markdown
              渲染、全文搜索、附件读取全部由 Rust 侧完成，WebView 只负责显示与输入。
            </p>
            <dl>
              <dt>数据源</dt>
              <dd>WebDAV（唯一来源）</dd>
              <dt>数据目录</dt>
              <dd>{stats?.dataDir}</dd>
              <dt>数据库</dt>
              <dd>{stats?.workDb}</dd>
              <dt>数据库大小</dt>
              <dd>{((stats?.workDbBytes ?? 0) / 1024 / 1024).toFixed(2)} MB</dd>
              <dt>内容</dt>
              <dd>
                {stats?.notes} 笔记 · {stats?.folders} 笔记本 ·{" "}
                {stats?.resources} 附件
              </dd>
              <dt>上次同步</dt>
              <dd>
                {syncStatus?.lastSyncAt
                  ? new Date(syncStatus.lastSyncAt).toLocaleString("zh-CN")
                  : "尚未同步"}
              </dd>
            </dl>
            <p className="modal-hint">
              快捷键：Ctrl+N 新建 · Ctrl+S 保存并同步 · Ctrl+F 搜索 · Ctrl+E 切换源码 / 打字机
              <br />
              右键笔记卡片：导出为 PDF · 删除笔记
            </p>
            <button className="primary-btn" onClick={() => setShowAbout(false)}>
              关闭
            </button>
          </div>
        </div>
      )}

      {folderDraft && (
        <div
          className="modal-backdrop"
          onClick={() => setFolderDraft(null)}
        >
          <form
            className="modal prompt-dialog"
            onClick={(e) => e.stopPropagation()}
            onSubmit={(e) => {
              e.preventDefault();
              void createFolder();
            }}
          >
            <h2>新建笔记本</h2>
            <input
              className="prompt-input"
              autoFocus
              value={folderDraft.title}
              placeholder="笔记本名称"
              onChange={(e) =>
                setFolderDraft({ ...folderDraft, title: e.target.value })
              }
              onKeyDown={(e) => {
                if (e.key === "Escape") setFolderDraft(null);
              }}
            />
            <div className="prompt-actions">
              <button
                type="button"
                className="ghost-btn"
                onClick={() => setFolderDraft(null)}
              >
                取消
              </button>
              <button
                type="submit"
                className="primary-btn"
                disabled={!folderDraft.title.trim()}
              >
                创建
              </button>
            </div>
          </form>
        </div>
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            {
              label: "导出为 PDF",
              icon: "⤓",
              onSelect: () => void exportPdf(menu.note),
            },
            {
              label: "删除笔记",
              icon: "🗑",
              danger: true,
              onSelect: () => setPendingDelete(menu.note),
            },
          ]}
        />
      )}

      {pendingDelete && (
        <div className="modal-backdrop" onClick={() => setPendingDelete(null)}>
          <div
            className="modal confirm-dialog"
            onClick={(e) => e.stopPropagation()}
          >
            <h2>删除这篇笔记？</h2>
            <p>
              「{pendingDelete.title || "无标题笔记"}」会被移入 Joplin
              的垃圾桶，下次同步时同步到云端；需要时可以再用官方客户端恢复。
            </p>
            <div className="prompt-actions">
              <button
                className="ghost-btn"
                onClick={() => setPendingDelete(null)}
              >
                取消
              </button>
              <button
                className="danger-btn"
                onClick={() => {
                  const target = pendingDelete;
                  setPendingDelete(null);
                  void deleteNote(target);
                }}
              >
                删除
              </button>
            </div>
          </div>
        </div>
      )}

      {printJob && <PrintSheet job={printJob} onReady={handlePrintReady} />}

      {printJob && <div className="pdf-busy">正在生成 PDF…</div>}
    </div>
  );
}
