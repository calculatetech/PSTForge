//! Safe, minimal ownership boundary around the libpff C API.

mod bindings;
mod catalog;

use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::ptr;

use bindings::{libpff_error_t, libpff_file_t, libpff_item_t};
use thiserror::Error;

const ERROR_BUFFER_SIZE: usize = 16 * 1024;
const MAX_FOLDERS: u64 = 1_000_000;

#[derive(Debug, Error)]
pub enum PffError {
    #[error("libpff {operation} failed: {detail}")]
    Native {
        operation: &'static str,
        detail: String,
    },
    #[error("libpff returned an invalid {field} value: {value}")]
    InvalidValue { field: &'static str, value: i64 },
    #[error("libpff returned no root folder")]
    MissingRootFolder,
    #[error("libpff returned a null pointer after {operation} succeeded")]
    NullPointer { operation: &'static str },
    #[error("source file descriptor is not available through /proc/self/fd")]
    ProcFileDescriptorUnavailable,
    #[error("catalog sink rejected {operation}: {detail}")]
    Sink {
        operation: &'static str,
        detail: String,
    },
    #[error("{field} exceeds the supported limit: {value} > {limit}")]
    LimitExceeded {
        field: &'static str,
        value: u64,
        limit: u64,
    },
    #[error("libpff streamed {actual} bytes for {field}, expected {expected}")]
    StreamSizeMismatch {
        field: &'static str,
        expected: u64,
        actual: u64,
    },
}

pub use catalog::{
    CatalogEvent, CatalogIssue, CatalogProvenance, CatalogSink, PropertyDescriptor, PropertyOwner,
    RawCatalog, STREAM_CHUNK_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPffMetadata {
    pub size: u64,
    pub content_type: Option<u8>,
    pub file_type: Option<u8>,
    pub encryption_type: Option<u8>,
    pub corrupted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryIssue {
    pub node_id: Option<u32>,
    pub operation: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawInventory {
    pub folders: u64,
    pub messages: u64,
    pub issues: Vec<InventoryIssue>,
}

pub fn library_version() -> String {
    // SAFETY: libpff returns a process-lifetime, NUL-terminated version string.
    let version = unsafe { bindings::libpff_get_version() };
    if version.is_null() {
        return "unknown".to_owned();
    }
    // SAFETY: the pointer contract above guarantees a valid C string.
    unsafe { CStr::from_ptr(version) }
        .to_string_lossy()
        .into_owned()
}

pub struct PffFile {
    raw: *mut libpff_file_t,
    is_open: bool,
}

impl PffFile {
    pub fn open_fd(fd: BorrowedFd<'_>) -> Result<Self, PffError> {
        let proc_path = format!("/proc/self/fd/{}", fd.as_raw_fd());
        if !std::path::Path::new(&proc_path).exists() {
            return Err(PffError::ProcFileDescriptorUnavailable);
        }
        let native_path = CString::new(proc_path).map_err(|error| PffError::Native {
            operation: "prepare source path",
            detail: error.to_string(),
        })?;

        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: raw and error are valid out-pointers initialized to null.
        let result = unsafe { bindings::libpff_file_initialize(&mut raw, &mut error) };
        if let Err(initialize_error) = check_one(result, error, "file initialize") {
            free_file(&mut raw);
            return Err(initialize_error);
        }
        if raw.is_null() {
            return Err(PffError::NullPointer {
                operation: "file initialize",
            });
        }

        // SAFETY: libpff_file_initialize succeeded and returned an owned file.
        let access_flags = unsafe { bindings::libpff_get_access_flags_read() };
        error = ptr::null_mut();
        // SAFETY: raw is initialized, native_path is NUL-terminated, and only
        // libpff's read access flag is provided.
        let result = unsafe {
            bindings::libpff_file_open(raw, native_path.as_ptr(), access_flags, &mut error)
        };
        if let Err(open_error) = check_one(result, error, "file open read-only") {
            free_file(&mut raw);
            return Err(open_error);
        }

        Ok(Self { raw, is_open: true })
    }

    pub fn metadata(&self) -> Result<RawPffMetadata, PffError> {
        let size = self.get_u64("get file size", bindings::libpff_file_get_size)?;
        let content_type =
            self.get_optional_u8("get content type", bindings::libpff_file_get_content_type)?;
        let file_type = self.get_optional_u8("get file type", bindings::libpff_file_get_type)?;
        let encryption_type = self.get_optional_u8(
            "get encryption type",
            bindings::libpff_file_get_encryption_type,
        )?;

        let mut error = ptr::null_mut();
        // SAFETY: self.raw is an open, owned libpff file pointer.
        let result = unsafe { bindings::libpff_file_is_corrupted(self.raw, &mut error) };
        let corrupted = match result {
            0 => {
                free_error(error);
                false
            }
            1 => {
                free_error(error);
                true
            }
            _ => return Err(native_error(error, "check corruption")),
        };

        Ok(RawPffMetadata {
            size,
            content_type,
            file_type,
            encryption_type,
            corrupted,
        })
    }

    pub fn inventory(&self) -> Result<RawInventory, PffError> {
        let mut root = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and root/error are initialized out-pointers.
        let result =
            unsafe { bindings::libpff_file_get_root_folder(self.raw, &mut root, &mut error) };
        check_one(result, error, "get root folder")?;
        if root.is_null() {
            return Err(PffError::MissingRootFolder);
        }

        let mut inventory = RawInventory::default();
        let mut stack = vec![PffItem { raw: root }];
        let mut visited = HashSet::new();

        while let Some(folder) = stack.pop() {
            if inventory.folders >= MAX_FOLDERS {
                return Err(PffError::InvalidValue {
                    field: "folder count",
                    value: i64::MAX,
                });
            }

            let node_id = match folder.identifier() {
                Ok(identifier) => Some(identifier),
                Err(error) => {
                    inventory
                        .issues
                        .push(issue(None, "get folder identifier", error));
                    None
                }
            };
            if node_id.is_some_and(|identifier| !visited.insert(identifier)) {
                inventory.issues.push(InventoryIssue {
                    node_id,
                    operation: "traverse folder",
                    message: "duplicate or cyclic folder identifier".to_owned(),
                });
                continue;
            }

            inventory.folders += 1;
            match folder.sub_message_count() {
                Ok(count) => {
                    inventory.messages =
                        inventory
                            .messages
                            .checked_add(count)
                            .ok_or(PffError::InvalidValue {
                                field: "message count",
                                value: i64::MAX,
                            })?;
                }
                Err(error) => inventory
                    .issues
                    .push(issue(node_id, "count folder messages", error)),
            }

            let sub_folder_count = match folder.sub_folder_count() {
                Ok(count) => count,
                Err(error) => {
                    inventory
                        .issues
                        .push(issue(node_id, "count subfolders", error));
                    continue;
                }
            };
            let pending = u64::try_from(stack.len()).map_err(|_| PffError::InvalidValue {
                field: "pending folder count",
                value: i64::MAX,
            })?;
            validate_folder_capacity(inventory.folders, pending, sub_folder_count)?;
            for index in (0..sub_folder_count).rev() {
                match folder.sub_folder(index) {
                    Ok(child) => stack.push(child),
                    Err(error) => inventory
                        .issues
                        .push(issue(node_id, "read subfolder", error)),
                }
            }
        }

        Ok(inventory)
    }

    fn get_u64(
        &self,
        operation: &'static str,
        function: unsafe extern "C" fn(
            *mut libpff_file_t,
            *mut u64,
            *mut *mut libpff_error_t,
        ) -> i32,
    ) -> Result<u64, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and value/error are initialized out-pointers.
        let result = unsafe { function(self.raw, &mut value, &mut error) };
        check_one(result, error, operation)?;
        Ok(value)
    }

    fn get_optional_u8(
        &self,
        operation: &'static str,
        function: unsafe extern "C" fn(
            *mut libpff_file_t,
            *mut u8,
            *mut *mut libpff_error_t,
        ) -> i32,
    ) -> Result<Option<u8>, PffError> {
        let mut value = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and value/error are initialized out-pointers.
        let result = unsafe { function(self.raw, &mut value, &mut error) };
        match result {
            1 => {
                free_error(error);
                Ok(Some(value))
            }
            0 => {
                free_error(error);
                Ok(None)
            }
            _ => Err(native_error(error, operation)),
        }
    }
}

impl Drop for PffFile {
    fn drop(&mut self) {
        if self.is_open && !self.raw.is_null() {
            let mut error = ptr::null_mut();
            // SAFETY: self.raw is owned and open. Drop cannot report close errors.
            unsafe {
                bindings::libpff_file_close(self.raw, &mut error);
            }
            free_error(error);
            self.is_open = false;
        }
        free_file(&mut self.raw);
    }
}

struct PffItem {
    raw: *mut libpff_item_t,
}

impl PffItem {
    fn identifier(&self) -> Result<u32, PffError> {
        let mut identifier = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is an owned, valid item and outputs are initialized.
        let result =
            unsafe { bindings::libpff_item_get_identifier(self.raw, &mut identifier, &mut error) };
        check_one(result, error, "get item identifier")?;
        Ok(identifier)
    }

    fn sub_folder_count(&self) -> Result<u64, PffError> {
        self.get_count(
            "get number of subfolders",
            bindings::libpff_folder_get_number_of_sub_folders,
        )
    }

    fn sub_message_count(&self) -> Result<u64, PffError> {
        self.get_count(
            "get number of submessages",
            bindings::libpff_folder_get_number_of_sub_messages,
        )
    }

    fn get_count(
        &self,
        operation: &'static str,
        function: unsafe extern "C" fn(
            *mut libpff_item_t,
            *mut i32,
            *mut *mut libpff_error_t,
        ) -> i32,
    ) -> Result<u64, PffError> {
        let mut count = 0;
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and count/error are initialized out-pointers.
        let result = unsafe { function(self.raw, &mut count, &mut error) };
        check_one(result, error, operation)?;
        u64::try_from(count).map_err(|_| PffError::InvalidValue {
            field: "item count",
            value: i64::from(count),
        })
    }

    fn sub_folder(&self, index: u64) -> Result<Self, PffError> {
        let index = i32::try_from(index).map_err(|_| PffError::InvalidValue {
            field: "subfolder index",
            value: i64::MAX,
        })?;
        let mut raw = ptr::null_mut();
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is valid and raw/error are initialized out-pointers.
        let result = unsafe {
            bindings::libpff_folder_get_sub_folder(self.raw, index, &mut raw, &mut error)
        };
        check_one(result, error, "get subfolder")?;
        if raw.is_null() {
            return Err(PffError::Native {
                operation: "get subfolder",
                detail: "libpff returned a null item".to_owned(),
            });
        }
        Ok(Self { raw })
    }
}

impl Drop for PffItem {
    fn drop(&mut self) {
        if self.raw.is_null() {
            return;
        }
        let mut error = ptr::null_mut();
        // SAFETY: self.raw is owned by this wrapper and freed exactly once.
        unsafe {
            bindings::libpff_item_free(&mut self.raw, &mut error);
        }
        free_error(error);
    }
}

fn issue(node_id: Option<u32>, operation: &'static str, error: PffError) -> InventoryIssue {
    InventoryIssue {
        node_id,
        operation,
        message: error.to_string(),
    }
}

fn validate_folder_capacity(processed: u64, pending: u64, children: u64) -> Result<(), PffError> {
    let projected = processed
        .checked_add(pending)
        .and_then(|count| count.checked_add(children))
        .ok_or(PffError::InvalidValue {
            field: "folder count",
            value: i64::MAX,
        })?;
    if projected > MAX_FOLDERS {
        return Err(PffError::InvalidValue {
            field: "folder count",
            value: i64::try_from(projected).map_or(i64::MAX, |value| value),
        });
    }
    Ok(())
}

fn check_one(
    result: i32,
    error: *mut libpff_error_t,
    operation: &'static str,
) -> Result<(), PffError> {
    if result == 1 {
        free_error(error);
        Ok(())
    } else {
        Err(native_error(error, operation))
    }
}

fn native_error(error: *mut libpff_error_t, operation: &'static str) -> PffError {
    let detail = if error.is_null() {
        "no native error details".to_owned()
    } else {
        let mut buffer = vec![0_u8; ERROR_BUFFER_SIZE];
        // SAFETY: error is a live libpff error and buffer is writable.
        let written = unsafe {
            bindings::libpff_error_backtrace_sprint(error, buffer.as_mut_ptr().cast(), buffer.len())
        };
        if written > 0 {
            let length = usize::try_from(written)
                .map_or(buffer.len().saturating_sub(1), |value| value)
                .min(buffer.len().saturating_sub(1));
            let content_length = buffer[..length]
                .iter()
                .position(|byte| *byte == 0)
                .map_or(length, |position| position);
            String::from_utf8_lossy(&buffer[..content_length])
                .trim()
                .to_owned()
        } else {
            "native error details unavailable".to_owned()
        }
    };
    free_error(error);
    PffError::Native { operation, detail }
}

fn free_error(mut error: *mut libpff_error_t) {
    if error.is_null() {
        return;
    }
    // SAFETY: error is owned by the caller under libpff's error convention.
    unsafe { bindings::libpff_error_free(&mut error) };
}

fn free_file(file: &mut *mut libpff_file_t) {
    if (*file).is_null() {
        return;
    }
    let mut error = ptr::null_mut();
    // SAFETY: file is an initialized owned pointer and is freed exactly once.
    unsafe {
        bindings::libpff_file_free(file, &mut error);
    }
    free_error(error);
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::{MAX_FOLDERS, check_one, library_version, validate_folder_capacity};

    #[test]
    fn linked_library_meets_the_supported_floor() {
        let version = library_version();
        assert!(version.as_str() >= "20180714", "version was {version}");
    }

    #[test]
    fn native_return_mapping_rejects_non_success() {
        assert!(check_one(1, ptr::null_mut(), "test").is_ok());
        assert!(check_one(0, ptr::null_mut(), "test").is_err());
        assert!(check_one(-1, ptr::null_mut(), "test").is_err());
    }

    #[test]
    fn folder_capacity_rejects_overflow_and_excessive_fanout() {
        assert!(validate_folder_capacity(1, 2, 3).is_ok());
        assert!(validate_folder_capacity(MAX_FOLDERS, 0, 1).is_err());
        assert!(validate_folder_capacity(u64::MAX, 1, 0).is_err());
    }
}
