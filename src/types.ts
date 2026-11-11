/** Custom MIME type used to carry a note id while dragging it onto a notebook. */
export const NOTE_DRAG_TYPE = "application/x-joplin-rs-note";

export interface Folder {
  id: string;
  title: string;
  parentId: string;
  noteCount: number;
  updatedTime: number;
}

export interface NoteSummary {
  id: string;
  title: string;
  parentId: string;
  createdTime: number;
  updatedTime: number;
  isTodo: number;
  todoCompleted: number;
  excerpt: string;
}

export interface Note {
  id: string;
  title: string;
  body: string;
  parentId: string;
  createdTime: number;
  updatedTime: number;
  isTodo: number;
  todoCompleted: number;
  markupLanguage: number;
  tags: string[];
}

export interface Tag {
  id: string;
  title: string;
  noteCount: number;
}

export interface OutlineItem {
  level: number;
  text: string;
  anchor: string;
}

export interface RenderedNote {
  html: string;
  wordCount: number;
  charCount: number;
  outline: OutlineItem[];
}

export interface SearchHit extends NoteSummary {
  score: number;
}

export interface Stats {
  notes: number;
  folders: number;
  resources: number;
  tags: number;
  /** The app's own data directory. WebDAV is the only data source. */
  dataDir: string;
  workDb: string;
  workDbBytes: number;
  syncedAt: number;
}

export interface Bootstrap {
  ok: boolean;
  error: string | null;
  dataDir: string;
  /** Application version, rendered in the status bar. */
  version: string;
  stats: Stats | null;
}

export interface ResourceData {
  base64: string;
  mime: string;
}

/** A file registered as a Joplin resource; the note body stores `:/<id>`. */
export interface CreatedResource {
  id: string;
  mime: string;
  size: number;
}

export type ViewMode = "source" | "wysiwyg";
export type Selection =
  | { kind: "all" }
  | { kind: "folder"; id: string }
  | { kind: "tag"; id: string }
  | { kind: "search"; query: string };

export interface SyncConfig {
  url: string;
  username: string;
  password: string;
  allowInvalidCerts: boolean;
  autoSync: boolean;
  intervalMinutes: number;
  lastSyncAt: number;
  lastMessage: string;
}

export interface SyncStatus {
  configured: boolean;
  url: string;
  username: string;
  autoSync: boolean;
  intervalMinutes: number;
  lastSyncAt: number;
  lastMessage: string;
}

export interface SyncProgress {
  phase: string;
  current: number;
  total: number;
  message: string;
}

export interface SyncResult {
  dryRun: boolean;
  downloaded: number;
  uploaded: number;
  deletedRemote: number;
  deletedLocal: number;
  skipped: number;
  resourcesDown: number;
  resourcesUp: number;
  unsupported: number;
  remoteItems: number;
  localItems: number;
  errors: string[];
  finishedAt: number;
  message: string;
}

/** A heading of the note being exported, in document order. `anchor` is the id
 *  the print sheet gives it; WebView2 turns the matching in-page link into a
 *  named destination, which is what the PDF bookmark points at. */
export interface PdfHeading {
  level: number;
  title: string;
  anchor: string;
}
