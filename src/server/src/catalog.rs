//! The cakes a round can be played on, by id.

use std::collections::BTreeMap;
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cake_core::cakes::{CakeError, CakeModel};
use cake_core::sim::Cake;

const CAKE_EXTENSION: &str = "cake";

/// Each cake built once, for every round's worlds to share.
pub struct Catalog {
    cakes: BTreeMap<String, Arc<Cake>>,
}

#[derive(Debug)]
pub enum CatalogError {
    Read { path: PathBuf, error: io::Error },
    Decode { path: PathBuf, error: CakeError },
    Empty { dir: PathBuf },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Read { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Decode { path, error } => write!(f, "{}: not a playable cake: {error:?}", path.display()),
            Self::Empty { dir } => write!(f, "{}: no .{CAKE_EXTENSION} files", dir.display()),
        }
    }
}

impl Catalog {
    /// Every `<id>.cake` file in `dir`.
    ///
    /// # Errors
    /// [`CatalogError`] when `dir` or a cake in it cannot be read, a cake is not playable, or
    /// `dir` holds no cakes.
    #[expect(clippy::missing_panics_doc, reason = "a path with an extension has a stem")]
    pub fn load(dir: &Path) -> Result<Self, CatalogError> {
        fn read_error(path: &Path) -> impl FnOnce(io::Error) -> CatalogError {
            let path = path.to_path_buf();
            move |error| CatalogError::Read { path, error }
        }
        let mut cakes = BTreeMap::new();
        for entry in dir.read_dir().map_err(read_error(dir))? {
            let path = entry.map_err(read_error(dir))?.path();
            if path.extension().is_none_or(|extension| extension != CAKE_EXTENSION) {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(read_error(&path))?;
            let model =
                CakeModel::decode(&bytes).map_err(|error| CatalogError::Decode { path: path.clone(), error })?;
            let id = path.file_stem().expect("a path with an extension has a stem").to_string_lossy().into_owned();
            cakes.insert(id, model.into());
        }
        if cakes.is_empty() {
            return Err(CatalogError::Empty { dir: dir.to_path_buf() });
        }
        Ok(Self { cakes })
    }

    /// A catalog of the given cakes.
    ///
    /// # Panics
    /// When `cakes` is empty.
    pub fn create(cakes: impl IntoIterator<Item = (String, CakeModel)>) -> Self {
        let cakes: BTreeMap<_, _> = cakes.into_iter().map(|(id, model)| (id, model.into())).collect();
        assert!(!cakes.is_empty());
        Self { cakes }
    }

    #[must_use]
    pub const fn get_len(&self) -> usize {
        self.cakes.len()
    }

    #[must_use]
    pub fn find(&self, id: &str) -> Option<&Arc<Cake>> {
        self.cakes.get(id)
    }

    /// The id of a cake chosen at random, each equally likely.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "a catalog is never empty")]
    #[expect(clippy::cast_possible_truncation, reason = "keeps the random hash's low bits")]
    pub fn pick_random_id(&self) -> &str {
        let idx = RandomState::new().hash_one(()) as usize % self.cakes.len();
        self.cakes.keys().nth(idx).expect("the index lies within the catalog")
    }
}
