//! L3 file import and export.
//!
//! Model import reads faceted geometry (STL, OBJ) into a [`Mesh`]. Every reader treats its
//! input as hostile: sizes and counts are capped, numbers must be finite and indices are
//! range-checked, so a bad file is an [`ImportError`], never a panic.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![allow(non_snake_case)]

pub mod obj;
pub mod stl;

use std::io::Read as _;
use std::path::Path;

use CosmicCAM_geom::{Mesh, MeshError};
use thiserror::Error;

/// Largest model file read.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Most triangles in one imported model.
pub const MAX_TRIANGLES: usize = 50_000_000;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ImportError {
    #[error("unsupported file type `{0}`; CosmicCAM imports .stl and .obj")]
    UnsupportedFormat(String),
    #[error("could not read {path}: {message}")]
    Io { path: String, message: String },
    #[error("file is larger than the {} MiB limit", MAX_FILE_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("model has more than {MAX_TRIANGLES} triangles")]
    TooManyTriangles,
    #[error("{format} line {line}: {message}")]
    Syntax { format: &'static str, line: usize, message: String },
    #[error("{0}")]
    Malformed(String),
    #[error("the file contains no triangles")]
    Empty,
}

impl From<MeshError> for ImportError {
    fn from(e: MeshError) -> Self {
        Self::Malformed(e.to_string())
    }
}

/// Model file formats CosmicCAM reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    Stl,
    Obj,
}

impl ModelFormat {
    pub const ALL: [Self; 2] = [Self::Stl, Self::Obj];

    pub fn id(self) -> &'static str {
        match self {
            Self::Stl => "stl",
            Self::Obj => "obj",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stl => "STL",
            Self::Obj => "Wavefront OBJ",
        }
    }

    /// File extensions, lower case, without the dot.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Stl => &["stl"],
            Self::Obj => &["obj"],
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id() == id)
    }

    /// The format a file name's extension names, ignoring case.
    pub fn from_path(path: &Path) -> Result<Self, ImportError> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|f| f.extensions().contains(&ext.as_str()))
            .ok_or_else(|| ImportError::UnsupportedFormat(if ext.is_empty() { "(none)".into() } else { ext }))
    }
}

/// Unit a model file was authored in. STL and OBJ carry no units, so the caller says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Units {
    Millimetres,
    Centimetres,
    Metres,
    Inches,
}

impl Units {
    pub const IDS: &'static [&'static str] = &["mm", "cm", "m", "in"];

    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "mm" => Self::Millimetres,
            "cm" => Self::Centimetres,
            "m" => Self::Metres,
            "in" => Self::Inches,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Millimetres => "mm",
            Self::Centimetres => "cm",
            Self::Metres => "m",
            Self::Inches => "in",
        }
    }

    /// Millimetres per unit.
    pub fn to_mm(self) -> f32 {
        match self {
            Self::Millimetres => 1.0,
            Self::Centimetres => 10.0,
            Self::Metres => 1000.0,
            Self::Inches => 25.4,
        }
    }
}

/// Parses a model file's bytes.
pub fn read_mesh(bytes: &[u8], format: ModelFormat) -> Result<Mesh, ImportError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(ImportError::TooLarge);
    }
    let mesh = match format {
        ModelFormat::Stl => stl::read(bytes)?,
        ModelFormat::Obj => obj::read(bytes)?,
    };
    if mesh.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(mesh)
}

/// Reads and parses a model file, choosing the format by extension.
pub fn import_mesh(path: &Path, units: Units) -> Result<(Mesh, ModelFormat), ImportError> {
    let format = ModelFormat::from_path(path)?;
    let io = |e: std::io::Error| ImportError::Io { path: path.display().to_string(), message: e.to_string() };
    let meta = std::fs::metadata(path).map_err(io)?;
    if !meta.is_file() {
        return Err(ImportError::Io { path: path.display().to_string(), message: "not a regular file".into() });
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(ImportError::TooLarge);
    }
    // Read through a cap in case the file grows while it is read.
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::fs::File::open(path).and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)).map_err(io)?;
    let mesh = read_mesh(&bytes, format)?;
    let mesh = if units == Units::Millimetres { mesh } else { mesh.scaled(units.to_mm())? };
    Ok((mesh, format))
}

/// Parses a model file's bytes, choosing the format by `name`'s extension. For files that
/// arrive without a path (a browser upload).
pub fn import_mesh_bytes(name: &str, bytes: &[u8], units: Units) -> Result<(Mesh, ModelFormat), ImportError> {
    let format = ModelFormat::from_path(Path::new(name))?;
    let mesh = read_mesh(bytes, format)?;
    let mesh = if units == Units::Millimetres { mesh } else { mesh.scaled(units.to_mm())? };
    Ok((mesh, format))
}

/// Parses an ASCII float, accepting only finite values.
pub(crate) fn finite(token: &str) -> Option<f32> {
    token.parse::<f32>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_comes_from_the_extension() {
        assert_eq!(ModelFormat::from_path(Path::new("a/b/Part.STL")), Ok(ModelFormat::Stl));
        assert_eq!(ModelFormat::from_path(Path::new("part.obj")), Ok(ModelFormat::Obj));
        assert!(
            matches!(ModelFormat::from_path(Path::new("part.step")), Err(ImportError::UnsupportedFormat(e)) if e == "step")
        );
        assert!(matches!(ModelFormat::from_path(Path::new("part")), Err(ImportError::UnsupportedFormat(_))));
    }

    #[test]
    fn empty_files_are_refused() {
        for f in ModelFormat::ALL {
            assert!(read_mesh(b"", f).is_err(), "{f:?}");
        }
        assert_eq!(read_mesh(b"solid x\nendsolid x\n", ModelFormat::Stl), Err(ImportError::Empty));
        assert_eq!(read_mesh(b"# nothing\nv 0 0 0\n", ModelFormat::Obj), Err(ImportError::Empty));
    }

    #[test]
    fn units_scale_to_millimetres() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join(format!("cosmiccam-io-units-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("tri.obj");
        std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")?;
        let (mesh, format) = import_mesh(&path, Units::Inches)?;
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(format, ModelFormat::Obj);
        assert_eq!(mesh.bounds().max.x, 25.4);
        Ok(())
    }

    #[test]
    fn missing_files_and_directories_are_io_errors() {
        assert!(matches!(
            import_mesh(Path::new("/no/such/dir/part.stl"), Units::Millimetres),
            Err(ImportError::Io { .. })
        ));
        let dir = std::env::temp_dir().join(format!("cosmiccam-io-dir-{}.stl", std::process::id()));
        let made = std::fs::create_dir_all(&dir).is_ok();
        let r = import_mesh(&dir, Units::Millimetres);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            !made || matches!(r, Err(ImportError::Io { ref message, .. }) if message == "not a regular file"),
            "{r:?}"
        );
    }

    /// Random and mangled bytes must come back as errors or meshes, never panics.
    #[test]
    fn garbage_never_panics() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let samples: [&[u8]; 6] = [
            b"solid a\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid a\n",
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 4 3\nf -1 -2 -3\n",
            b"f 1 2 3\nv nan 0 0\nv 1e39 0 0\nf 0 0 0\nf 99999999999 1 2\n",
            b"solid\nvertex 1 2\nfacet\nendloop\n",
            &[0u8; 84],
            &[0xffu8; 200],
        ];
        for sample in samples {
            for f in ModelFormat::ALL {
                let _ = read_mesh(sample, f);
                for _ in 0..200 {
                    let mut bytes = sample.to_vec();
                    for _ in 0..4 {
                        let len = bytes.len().max(1);
                        let i = (next() as usize) % len;
                        if let Some(b) = bytes.get_mut(i) {
                            *b = next() as u8;
                        }
                    }
                    bytes.truncate((next() as usize) % (bytes.len() + 1));
                    let _ = read_mesh(&bytes, f);
                }
            }
        }
    }
}
