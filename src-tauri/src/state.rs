use crate::{
    db::{Db, ProfileConfig},
    error::{Error, Result},
    search::SearchIndex,
};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;

pub struct AppState {
    /// Wrapped in Option so `reload` can drop the connection before reopening
    /// the database file (mandatory on Windows, where open files are locked).
    pub db: Mutex<Option<Db>>,
    pub cfg: RwLock<ProfileConfig>,
    pub index: RwLock<SearchIndex>,
    pub resource_ext: RwLock<HashMap<String, String>>,
    pub note_titles: RwLock<HashMap<String, String>>,
    pub init_error: RwLock<Option<String>>,
}

impl AppState {
    pub fn init() -> Result<Self> {
        let cfg = ProfileConfig::load()?;
        let db = Db::open(&cfg.work_db)?;
        let index = SearchIndex::build(db.all_for_index()?);
        let resource_ext = db.resource_ext_map()?;
        let note_titles = db.note_title_map()?;
        Ok(Self {
            db: Mutex::new(Some(db)),
            cfg: RwLock::new(cfg),
            index: RwLock::new(index),
            resource_ext: RwLock::new(resource_ext),
            note_titles: RwLock::new(note_titles),
            init_error: RwLock::new(None),
        })
    }

    pub fn fallback(err: &Error) -> Self {
        let layout = crate::paths::paths();
        let cfg = ProfileConfig {
            resources: layout.resources,
            work_db: layout.database,
            synced_at: 0,
        };
        Self {
            db: Mutex::new(None),
            cfg: RwLock::new(cfg),
            index: RwLock::new(SearchIndex::build(Vec::new())),
            resource_ext: RwLock::new(HashMap::new()),
            note_titles: RwLock::new(HashMap::new()),
            init_error: RwLock::new(Some(err.to_string())),
        }
    }

    pub fn with_db<T>(&self, f: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
        let guard = self.db.lock();
        let db = guard
            .as_ref()
            .ok_or_else(|| Error::msg("数据库尚未就绪，请重新打开应用"))?;
        f(db)
    }

    pub fn refresh_from_db(&self, db: &Db) -> Result<()> {
        *self.index.write() = SearchIndex::build(db.all_for_index()?);
        *self.resource_ext.write() = db.resource_ext_map()?;
        *self.note_titles.write() = db.note_title_map()?;
        Ok(())
    }

    /// Reopen the database and rebuild the in-memory indexes. The sync engine
    /// writes through its own connection, so without this the search index and
    /// attachment map would keep serving the pre-sync snapshot.
    pub fn reload(&self) -> Result<()> {
        let path = self.cfg.read().work_db.clone();
        *self.db.lock() = None;
        let db = Db::open(&path)?;
        self.refresh_from_db(&db)?;
        *self.db.lock() = Some(db);
        Ok(())
    }
}
