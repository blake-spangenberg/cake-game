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
            CatalogError::Read { path, error } => write!(f, "{}: {error}", path.display()),
            CatalogError::Decode { path, error } => write!(f, "{}: not a playable cake: {error:?}", path.display()),
            CatalogError::Empty { dir } => write!(f, "{}: no .{CAKE_EXTENSION} files", dir.display()),
        }
    }
}

impl Catalog {
    /// Every `<id>.cake` file in `dir`.
    pub fn load(dir: &Path) -> Result<Catalog, CatalogError> {
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
            let id = path.file_stem().unwrap().to_string_lossy().into_owned();
            cakes.insert(id, model.into());
        }
        if cakes.is_empty() {
            return Err(CatalogError::Empty { dir: dir.to_path_buf() });
        }
        Ok(Catalog { cakes })
    }

    /// A catalog of the given cakes, at least one.
    pub fn create(cakes: impl IntoIterator<Item = (String, CakeModel)>) -> Catalog {
        let cakes: BTreeMap<_, _> = cakes.into_iter().map(|(id, model)| (id, model.into())).collect();
        assert!(!cakes.is_empty());
        Catalog { cakes }
    }

    pub fn get_len(&self) -> usize {
        self.cakes.len()
    }

    /// The cake with `id`, or a random one for `random`.
    pub fn find(&self, id: &str) -> Option<(&str, &Arc<Cake>)> {
        if id == "random" {
            return Some(self.pick_random());
        }
        self.cakes.get_key_value(id).map(|(id, cake)| (id.as_str(), cake))
    }

    fn pick_random(&self) -> (&str, &Arc<Cake>) {
        let idx = RandomState::new().hash_one(()) as usize % self.cakes.len();
        let (id, cake) = self.cakes.iter().nth(idx).unwrap();
        (id, cake)
    }
}
