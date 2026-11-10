import { invoke } from "@tauri-apps/api/core";
import type {
  Bootstrap,
  Folder,
  Note,
  NoteSummary,
  PdfHeading,
  RenderedNote,
  ResourceData,
  SearchHit,
  Stats,
  SyncConfig,
  SyncResult,
  SyncStatus,
  Tag,
} from "./types";

/** Typed IPC layer. Every byte of persistence, parsing and search stays in Rust.
 *
 * Tauri 2 maps Rust `snake_case` command arguments to `camelCase` JS keys by
 * default (see tauri-macros `ArgumentCase::Camel`). Passing `folder_id` from JS
 * silently becomes `None` on the Rust side because these args are `Option<_>`,
 * which makes folder/tag filtering look like a no-op. Always use camelCase here.
 */
export const api = {
  bootstrap: () => invoke<Bootstrap>("bootstrap"),

  getStats: () => invoke<Stats>("get_stats"),

  listFolders: () => invoke<Folder[]>("list_folders"),
  listTags: () => invoke<Tag[]>("list_tags"),
  listNotes: (args: {
    folderId?: string | null;
    tagId?: string | null;
    limit?: number;
    offset?: number;
  }) => invoke<NoteSummary[]>("list_notes", args),

  getNote: (id: string) => invoke<Note>("get_note", { id }),
  renderNote: (id: string) => invoke<RenderedNote>("render_note", { id }),
  renderMarkdown: (body: string) =>
    invoke<RenderedNote>("render_markdown", { body }),

  saveNote: (id: string, title: string, body: string) =>
    invoke<NoteSummary>("save_note", { id, title, body }),
  createNote: (folderId: string, title: string) =>
    invoke<Note>("create_note", { folderId, title }),
  moveNote: (id: string, folderId: string) =>
    invoke<NoteSummary>("move_note", { id, folderId }),
  deleteNote: (id: string) => invoke<void>("delete_note", { id }),
  /** Writes whatever the window is showing to `path` — the caller puts the note
   *  on screen first (see `PrintSheet`), so no note id is involved. `headings`
   *  turn into the PDF's bookmark tree. */
  exportNotePdf: (path: string, headings: PdfHeading[]) =>
    invoke<void>("export_note_pdf", { path, headings }),
  createFolder: (title: string, parentId: string) =>
    invoke<Folder>("create_folder", { title, parentId }),
  renameFolder: (id: string, title: string) =>
    invoke<void>("rename_folder", { id, title }),

  searchNotes: (query: string, limit?: number) =>
    invoke<SearchHit[]>("search_notes", { query, limit }),
  readResource: (id: string) =>
    invoke<ResourceData>("read_resource", { id }),

  syncGetConfig: () => invoke<SyncConfig>("sync_get_config"),
  syncStatus: () => invoke<SyncStatus>("sync_status"),
  syncSetConfig: (config: SyncConfig) =>
    invoke<SyncStatus>("sync_set_config", { config }),
  syncTest: (config: SyncConfig) =>
    invoke<string>("sync_test", { config }),
  syncPreview: () => invoke<SyncResult>("sync_preview"),
  syncRun: () => invoke<SyncResult>("sync_run"),
  appDataDir: () => invoke<string>("app_data_dir"),
  appQuit: () => invoke<void>("app_quit"),
};
