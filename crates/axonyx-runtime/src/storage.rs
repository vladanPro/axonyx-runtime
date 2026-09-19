use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::backend::{AxRuntimeError, AxRuntimeResult};
use crate::server::{AxFileRef, AxFileStorage, AxIncomingFile};

const MAX_CAPABILITY_NAME_BYTES: usize = 64;
const MAX_FILE_NAME_BYTES: usize = 255;
const MAX_CONTENT_TYPE_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AxStorageAccess {
    Read,
    Write,
    ReadWrite,
}

impl AxStorageAccess {
    pub fn can_read(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }

    pub fn can_write(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

#[derive(Debug, Error)]
pub enum AxStorageError {
    #[error("storage capability name is invalid")]
    InvalidCapabilityName,
    #[error("storage file limit must be greater than zero")]
    InvalidFileLimit,
    #[error("storage capability does not allow `{operation}`")]
    AccessDenied { operation: &'static str },
    #[error("storage capability `{name}` is already registered")]
    DuplicateCapability { name: String },
    #[error("storage capability `{name}` is not registered")]
    UnknownCapability { name: String },
    #[error("storage file name is invalid")]
    InvalidFileName,
    #[error("storage content type metadata is invalid")]
    InvalidContentType,
    #[error("storage file exceeds the {max_bytes} byte limit ({actual_bytes} bytes received)")]
    FileTooLarge { max_bytes: u64, actual_bytes: u64 },
    #[error("file reference belongs to a different storage capability")]
    StorageMismatch,
    #[error("file reference id is invalid")]
    InvalidFileId,
    #[error("stored file does not match its reference")]
    IntegrityMismatch,
    #[error("storage operation `{operation}` failed")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug)]
pub struct AxCapabilityStorage {
    name: String,
    root: Dir,
    max_file_bytes: u64,
    access: AxStorageAccess,
}

impl AxCapabilityStorage {
    pub fn open(
        name: impl Into<String>,
        root: impl AsRef<Path>,
        max_file_bytes: u64,
    ) -> Result<Self, AxStorageError> {
        Self::open_with_access(name, root, max_file_bytes, AxStorageAccess::ReadWrite)
    }

    pub fn open_with_access(
        name: impl Into<String>,
        root: impl AsRef<Path>,
        max_file_bytes: u64,
        access: AxStorageAccess,
    ) -> Result<Self, AxStorageError> {
        let name = name.into();
        validate_capability_name(&name)?;
        if max_file_bytes == 0 {
            return Err(AxStorageError::InvalidFileLimit);
        }

        std::fs::create_dir_all(root.as_ref()).map_err(|source| AxStorageError::Io {
            operation: "open-root",
            source,
        })?;
        let root = Dir::open_ambient_dir(root.as_ref(), ambient_authority()).map_err(|source| {
            AxStorageError::Io {
                operation: "open-root",
                source,
            }
        })?;

        Ok(Self {
            name,
            root,
            max_file_bytes,
            access,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }

    pub fn access(&self) -> AxStorageAccess {
        self.access
    }

    pub fn store(&self, file: &AxIncomingFile) -> Result<AxFileRef, AxStorageError> {
        self.store_bytes(&file.file_name, file.content_type.as_deref(), &file.bytes)
    }

    pub fn store_bytes(
        &self,
        file_name: &str,
        content_type: Option<&str>,
        bytes: &[u8],
    ) -> Result<AxFileRef, AxStorageError> {
        if !self.access.can_write() {
            return Err(AxStorageError::AccessDenied { operation: "write" });
        }
        validate_file_name(file_name)?;
        validate_content_type(content_type)?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if size > self.max_file_bytes {
            return Err(AxStorageError::FileTooLarge {
                max_bytes: self.max_file_bytes,
                actual_bytes: size,
            });
        }

        let id = sha256_hex(bytes);
        let path = object_path(&id, file_name)?;
        let parent = path.parent().ok_or(AxStorageError::InvalidFileName)?;
        self.root
            .create_dir_all(parent)
            .map_err(|source| AxStorageError::Io {
                operation: "create-object-directory",
                source,
            })?;

        if self
            .root
            .try_exists(&path)
            .map_err(|source| AxStorageError::Io {
                operation: "inspect-object",
                source,
            })?
        {
            let existing = self.root.read(&path).map_err(|source| AxStorageError::Io {
                operation: "read-existing-object",
                source,
            })?;
            if existing != bytes {
                return Err(AxStorageError::IntegrityMismatch);
            }
        } else {
            self.root
                .write(&path, bytes)
                .map_err(|source| AxStorageError::Io {
                    operation: "write-object",
                    source,
                })?;
        }

        Ok(AxFileRef {
            id,
            storage: self.name.clone(),
            file_name: file_name.to_string(),
            content_type: content_type.map(str::to_owned),
            size,
        })
    }

    pub fn read(&self, file_ref: &AxFileRef) -> Result<Vec<u8>, AxStorageError> {
        if !self.access.can_read() {
            return Err(AxStorageError::AccessDenied { operation: "read" });
        }
        self.validate_ref(file_ref)?;
        let path = object_path(&file_ref.id, &file_ref.file_name)?;
        let bytes = self.root.read(path).map_err(|source| AxStorageError::Io {
            operation: "read-object",
            source,
        })?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if size != file_ref.size || sha256_hex(&bytes) != file_ref.id {
            return Err(AxStorageError::IntegrityMismatch);
        }
        Ok(bytes)
    }

    fn validate_ref(&self, file_ref: &AxFileRef) -> Result<(), AxStorageError> {
        if file_ref.storage != self.name {
            return Err(AxStorageError::StorageMismatch);
        }
        validate_file_id(&file_ref.id)?;
        validate_file_name(&file_ref.file_name)?;
        validate_content_type(file_ref.content_type.as_deref())
    }
}

#[derive(Debug, Default)]
pub struct AxStorageRegistry {
    capabilities: BTreeMap<String, AxCapabilityStorage>,
}

impl AxStorageRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, storage: AxCapabilityStorage) -> Result<(), AxStorageError> {
        let name = storage.name().to_string();
        if self.capabilities.contains_key(&name) {
            return Err(AxStorageError::DuplicateCapability { name });
        }
        self.capabilities.insert(name, storage);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&AxCapabilityStorage, AxStorageError> {
        self.capabilities
            .get(name)
            .ok_or_else(|| AxStorageError::UnknownCapability {
                name: name.to_string(),
            })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.capabilities.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    pub fn len(&self) -> usize {
        self.capabilities.len()
    }
}

impl AxFileStorage for AxStorageRegistry {
    fn save_file(&self, capability: &str, file: &AxIncomingFile) -> AxRuntimeResult<AxFileRef> {
        self.get(capability)
            .and_then(|storage| storage.store(file))
            .map_err(|error| AxRuntimeError::message(error.to_string()))
    }
}

fn validate_capability_name(value: &str) -> Result<(), AxStorageError> {
    if value.is_empty()
        || value.len() > MAX_CAPABILITY_NAME_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(AxStorageError::InvalidCapabilityName);
    }
    Ok(())
}

fn validate_file_name(value: &str) -> Result<(), AxStorageError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > MAX_FILE_NAME_BYTES
        || value.chars().any(char::is_control)
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        return Err(AxStorageError::InvalidFileName);
    }
    Ok(())
}

fn validate_content_type(value: Option<&str>) -> Result<(), AxStorageError> {
    if value.is_some_and(|value| {
        value.is_empty()
            || value.len() > MAX_CONTENT_TYPE_BYTES
            || value.chars().any(char::is_control)
    }) {
        return Err(AxStorageError::InvalidContentType);
    }
    Ok(())
}

fn validate_file_id(value: &str) -> Result<(), AxStorageError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(AxStorageError::InvalidFileId);
    }
    Ok(())
}

fn object_path(id: &str, file_name: &str) -> Result<PathBuf, AxStorageError> {
    validate_file_id(id)?;
    validate_file_name(file_name)?;
    Ok(Path::new("objects").join(&id[..2]).join(id).join(file_name))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

pub mod prelude {
    pub use crate::server::AxFileRef;

    pub use super::{AxCapabilityStorage, AxStorageAccess, AxStorageError, AxStorageRegistry};
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("axonyx-storage-{name}-{}", std::process::id()))
    }

    fn incoming(file_name: &str, bytes: &[u8]) -> AxIncomingFile {
        AxIncomingFile {
            field_name: "image".to_string(),
            file_name: file_name.to_string(),
            content_type: Some("image/png".to_string()),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn stores_and_reads_a_file_without_exposing_the_ambient_root() {
        let root = temp_root("round-trip");
        let storage = AxCapabilityStorage::open("media", &root, 1024)
            .expect("storage capability should open");

        let file_ref = storage
            .store(&incoming("cover.png", b"PNG"))
            .expect("incoming file should store");
        assert_eq!(storage.read(&file_ref).expect("file should read"), b"PNG");
        assert_eq!(file_ref.storage, "media");
        assert_eq!(file_ref.file_name, "cover.png");
        assert_eq!(file_ref.size, 3);
        let public_json = serde_json::to_string(&file_ref).expect("file ref should serialize");
        assert!(!public_json.contains(&root.display().to_string()));

        drop(storage);
        std::fs::remove_dir_all(root).expect("storage root should clean up");
    }

    #[test]
    fn rejects_unsafe_names_and_oversized_files_before_writing() {
        let root = temp_root("limits");
        let storage =
            AxCapabilityStorage::open("media", &root, 2).expect("storage capability should open");

        assert!(matches!(
            storage.store(&incoming("../secret.txt", b"x")),
            Err(AxStorageError::InvalidFileName)
        ));
        assert!(matches!(
            storage.store(&incoming("cover.png", b"PNG")),
            Err(AxStorageError::FileTooLarge { .. })
        ));
        assert!(!root.join("objects").exists());

        drop(storage);
        std::fs::remove_dir_all(root).expect("storage root should clean up");
    }

    #[test]
    fn rejects_a_reference_from_another_capability() {
        let first_root = temp_root("first");
        let second_root = temp_root("second");
        let first = AxCapabilityStorage::open("media", &first_root, 1024)
            .expect("first capability should open");
        let second = AxCapabilityStorage::open("private", &second_root, 1024)
            .expect("second capability should open");
        let file_ref = first
            .store(&incoming("cover.png", b"PNG"))
            .expect("incoming file should store");

        assert!(matches!(
            second.read(&file_ref),
            Err(AxStorageError::StorageMismatch)
        ));

        drop(first);
        drop(second);
        std::fs::remove_dir_all(first_root).expect("first root should clean up");
        std::fs::remove_dir_all(second_root).expect("second root should clean up");
    }

    #[test]
    fn enforces_access_and_registry_boundaries() {
        let read_root = temp_root("read-only");
        let write_root = temp_root("write-only");
        let read_only = AxCapabilityStorage::open_with_access(
            "public",
            &read_root,
            1024,
            AxStorageAccess::Read,
        )
        .expect("read capability should open");
        let write_only = AxCapabilityStorage::open_with_access(
            "uploads",
            &write_root,
            1024,
            AxStorageAccess::Write,
        )
        .expect("write capability should open");

        assert!(matches!(
            read_only.store(&incoming("cover.png", b"PNG")),
            Err(AxStorageError::AccessDenied { operation: "write" })
        ));
        let file_ref = write_only
            .store(&incoming("cover.png", b"PNG"))
            .expect("write capability should store");
        assert!(matches!(
            write_only.read(&file_ref),
            Err(AxStorageError::AccessDenied { operation: "read" })
        ));

        let mut registry = AxStorageRegistry::new();
        registry
            .register(read_only)
            .expect("first capability should register");
        assert_eq!(registry.names().collect::<Vec<_>>(), vec!["public"]);
        let duplicate = AxCapabilityStorage::open_with_access(
            "public",
            &write_root,
            1024,
            AxStorageAccess::Read,
        )
        .expect("duplicate capability should open before registration");
        assert!(matches!(
            registry.register(duplicate),
            Err(AxStorageError::DuplicateCapability { .. })
        ));
        assert!(matches!(
            registry.get("missing"),
            Err(AxStorageError::UnknownCapability { .. })
        ));

        drop(registry);
        drop(write_only);
        std::fs::remove_dir_all(read_root).expect("read root should clean up");
        std::fs::remove_dir_all(write_root).expect("write root should clean up");
    }
}
