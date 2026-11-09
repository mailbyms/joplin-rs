import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { NOTE_DRAG_TYPE } from "../types";
import type { Folder, Selection, Tag } from "../types";

interface Props {
  folders: Folder[];
  tags: Tag[];
  selection: Selection;
  totalNotes: number;
  syncConfigured: boolean;
  syncBusy: boolean;
  syncTitle: string;
  /** Rolling sync milestones, already joined with "。" */
  syncFeed: string;
  onSyncNow: () => void;
  onOpenSync: () => void;
  onSelect: (next: Selection) => void;
  onNewFolder: (parentId: string) => void;
  /** A note card was dropped on a notebook: move it there. */
  onMoveNote: (noteId: string, folderId: string) => void;
}

interface TreeNode extends Folder {
  children: TreeNode[];
  depth: number;
}

function buildTree(folders: Folder[]): TreeNode[] {
  const nodes = new Map<string, TreeNode>();
  for (const folder of folders) {
    nodes.set(folder.id, { ...folder, children: [], depth: 0 });
  }
  const roots: TreeNode[] = [];
  for (const folder of folders) {
    const node = nodes.get(folder.id)!;
    const parent = folder.parentId ? nodes.get(folder.parentId) : undefined;
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  const sort = (list: TreeNode[], depth: number) => {
    list.sort((a, b) => a.title.localeCompare(b.title, "zh-CN"));
    for (const node of list) {
      node.depth = depth;
      sort(node.children, depth + 1);
    }
  };
  sort(roots, 0);
  return roots;
}

function flatten(roots: TreeNode[], expanded: Set<string>): TreeNode[] {
  const out: TreeNode[] = [];
  const walk = (list: TreeNode[]) => {
    for (const node of list) {
      out.push(node);
      if (node.children.length && expanded.has(node.id)) walk(node.children);
    }
  };
  walk(roots);
  return out;
}

function RefreshIcon() {
  return (
    <svg
      className="foot-icon"
      viewBox="0 0 24 24"
      width="12"
      height="12"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <polyline points="21 4 21 9 16 9" />
      <polyline points="3 20 3 15 8 15" />
      <path d="M3.9 9a8 8 0 0 1 13.2-3L21 9" />
      <path d="M20.1 15a8 8 0 0 1-13.2 3L3 15" />
    </svg>
  );
}

/** One-pass marquee for the sync milestones: the text slides left at a constant
    30px/s until its tail is visible, then stops there. It never loops — a
    permanently moving strip next to the folder list is tiring to look at. */
function SyncTicker({ text }: { text: string }) {
  const wrap = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLSpanElement>(null);
  const [pass, setPass] = useState({ on: false, seconds: 4, shift: 0 });

  useEffect(() => {
    const measure = () => {
      const box = wrap.current;
      const span = inner.current;
      if (!box || !span) return;
      const overflow = span.scrollWidth - box.clientWidth;
      setPass({
        on: overflow > 1,
        // Constant speed, duration derived from the distance to travel.
        seconds: Math.max(2, Math.round(overflow / 30)),
        shift: Math.max(0, overflow),
      });
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (wrap.current) observer.observe(wrap.current);
    return () => observer.disconnect();
  }, [text]);

  return (
    <div
      className="sync-ticker"
      ref={wrap}
      title={text}
      role="status"
      aria-label={text}
    >
      <div
        key={text}
        className={`ticker-track ${pass.on ? "scrolling" : ""}`}
        aria-hidden="true"
        style={
          pass.on
            ? ({
                animationDuration: `${pass.seconds}s`,
                "--ticker-shift": `-${pass.shift}px`,
              } as CSSProperties)
            : undefined
        }
      >
        <span className="ticker-text" ref={inner}>
          {text}
        </span>
      </div>
    </div>
  );
}

export default function Sidebar({
  folders,
  tags,
  selection,
  totalNotes,
  syncConfigured,
  syncBusy,
  syncTitle,
  syncFeed,
  onSyncNow,
  onOpenSync,
  onSelect,
  onNewFolder,
  onMoveNote,
}: Props) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const roots = useMemo(() => buildTree(folders), [folders]);
  const visible = useMemo(() => flatten(roots, expanded), [roots, expanded]);

  const toggle = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <aside className="sidebar">
      <nav className="sidebar-scroll">
        <button
          className={`nav-item ${selection.kind === "all" ? "active" : ""}`}
          onClick={() => onSelect({ kind: "all" })}
        >
          <span className="nav-icon">▤</span>
          <span>全部笔记</span>
          <span className="nav-count">{totalNotes}</span>
        </button>

        <div className="section-head">
          <span>笔记本</span>
          <button
            className="icon-btn"
            title="新建顶层笔记本"
            onClick={() => onNewFolder("")}
          >
            ＋
          </button>
        </div>
        {visible.map((folder) => {
          const isActive =
            selection.kind === "folder" && selection.id === folder.id;
          const isDropTarget = dropTarget === folder.id;
          return (
            <button
              key={folder.id}
              className={`nav-item ${isActive ? "active" : ""} ${
                isDropTarget ? "drop-target" : ""
              }`}
              style={{ paddingLeft: 12 + folder.depth * 14 }}
              onClick={() => onSelect({ kind: "folder", id: folder.id })}
              // Dropping a note card here moves the note into this notebook.
              onDragOver={(e) => {
                if (!e.dataTransfer.types.includes(NOTE_DRAG_TYPE)) return;
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                setDropTarget(folder.id);
              }}
              onDragLeave={() =>
                setDropTarget((cur) => (cur === folder.id ? null : cur))
              }
              onDrop={(e) => {
                e.preventDefault();
                setDropTarget(null);
                const noteId = e.dataTransfer.getData(NOTE_DRAG_TYPE);
                if (noteId) onMoveNote(noteId, folder.id);
              }}
            >
              <span
                className={`tree-arrow ${folder.children.length ? "" : "empty"}`}
                onClick={(e) => {
                  // The arrow used to swallow the click entirely, so clicking
                  // near the left edge only expanded the tree and never changed
                  // the note list. Always select the folder as well.
                  e.stopPropagation();
                  if (folder.children.length) toggle(folder.id);
                  onSelect({ kind: "folder", id: folder.id });
                }}
              >
                {folder.children.length
                  ? expanded.has(folder.id)
                    ? "▾"
                    : "▸"
                  : "·"}
              </span>
              <span className="nav-label">{folder.title}</span>
              {folder.noteCount > 0 && (
                <span className="nav-count">{folder.noteCount}</span>
              )}
            </button>
          );
        })}

        {tags.length > 0 && (
          <>
            <div className="section-head">
              <span>标签</span>
            </div>
            {tags.map((tag) => (
              <button
                key={tag.id}
                className={`nav-item ${
                  selection.kind === "tag" && selection.id === tag.id
                    ? "active"
                    : ""
                }`}
                onClick={() => onSelect({ kind: "tag", id: tag.id })}
              >
                <span className="nav-icon">#</span>
                <span className="nav-label">{tag.title}</span>
                <span className="nav-count">{tag.noteCount}</span>
              </button>
            ))}
          </>
        )}
      </nav>

      {/* The top bar is gone, so WebDAV sync lives here: always reachable,
          never in the way of the editor. The last-sync stamp moved to the
          status bar at the bottom right. */}
      <footer className="sidebar-foot">
        <div className="foot-row">
          <button
            className={`foot-btn foot-sync ${syncBusy ? "busy" : ""} ${
              syncConfigured ? "" : "unconfigured"
            }`}
            onClick={onSyncNow}
            onContextMenu={(e) => {
              e.preventDefault();
              onOpenSync();
            }}
            title={
              syncConfigured
                ? `${syncTitle} · 点击立即同步，右键打开同步设置`
                : "尚未配置 WebDAV，点击填写同步设置"
            }
          >
            <RefreshIcon />
            {syncBusy ? "同步中" : "同步"}
          </button>
          <SyncTicker text={syncFeed} />
        </div>
      </footer>
    </aside>
  );
}
