import { useState, type RefObject } from "react";
import { NOTE_DRAG_TYPE } from "../types";
import type { NoteSummary } from "../types";

interface Props {
  notes: NoteSummary[];
  activeId: string | null;
  searchValue: string;
  searchRef: RefObject<HTMLInputElement | null>;
  onSearchChange: (value: string) => void;
  onSelect: (id: string) => void;
  onNew: () => void;
}

function formatTime(ts: number): string {
  if (!ts) return "";
  const date = new Date(ts);
  const now = new Date();
  if (date.toDateString() === now.toDateString()) {
    return date.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  }
  if (date.getFullYear() === now.getFullYear()) {
    return `${date.getMonth() + 1}月${date.getDate()}日`;
  }
  return date.toLocaleDateString("zh-CN");
}

export default function NoteList({
  notes,
  activeId,
  searchValue,
  searchRef,
  onSearchChange,
  onSelect,
  onNew,
}: Props) {
  // Cards are draggable so they can be dropped on a notebook in the sidebar;
  // the id travels in a custom MIME type, the plain-text payload is a fallback
  // for anything outside the app that accepts a drop.
  const [draggingId, setDraggingId] = useState<string | null>(null);
  return (
    <section className="note-list">
      <header className="list-head">
        <div className="list-search">
          <span className="search-icon">⌕</span>
          <input
            ref={searchRef}
            value={searchValue}
            onChange={(e) => onSearchChange(e.target.value)}
            placeholder="搜索  Ctrl+F"
            aria-label="搜索笔记"
          />
          {searchValue && (
            <button
              className="clear-btn"
              title="清除搜索"
              onClick={() => onSearchChange("")}
            >
              ×
            </button>
          )}
        </div>
        <button
          className="list-new-btn"
          title="新建笔记（Ctrl+N）"
          onClick={onNew}
        >
          ＋ 新建笔记
        </button>
      </header>
      <div className="list-scroll">
        {notes.length === 0 && (
          <div className="list-empty">这里还没有笔记</div>
        )}
        {notes.map((note) => (
          <button
            key={note.id}
            className={`note-card ${activeId === note.id ? "active" : ""} ${
              draggingId === note.id ? "dragging" : ""
            }`}
            onClick={() => onSelect(note.id)}
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData(NOTE_DRAG_TYPE, note.id);
              e.dataTransfer.setData("text/plain", note.title || "无标题笔记");
              e.dataTransfer.effectAllowed = "move";
              setDraggingId(note.id);
            }}
            onDragEnd={() => setDraggingId(null)}
          >
            <div className="card-top">
              <span className="card-title">
                {note.todoCompleted ? "☑ " : note.isTodo ? "☐ " : ""}
                {note.title || "无标题笔记"}
              </span>
              <span className="card-time">{formatTime(note.updatedTime)}</span>
            </div>
            <div className="card-excerpt">{note.excerpt || "空白笔记"}</div>
          </button>
        ))}
      </div>
    </section>
  );
}
