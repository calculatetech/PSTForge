//! Minimal checked-in bindings shared by libpff 20180714 and 20231205.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uchar};

#[repr(C)]
pub(crate) struct libpff_error_t {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct libpff_file_t {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct libpff_item_t {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub(crate) fn libpff_get_version() -> *const c_char;
    pub(crate) fn libpff_get_access_flags_read() -> c_int;
    pub(crate) fn libpff_error_free(error: *mut *mut libpff_error_t);
    pub(crate) fn libpff_error_backtrace_sprint(
        error: *mut libpff_error_t,
        string: *mut c_char,
        size: usize,
    ) -> c_int;

    pub(crate) fn libpff_file_initialize(
        file: *mut *mut libpff_file_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_free(
        file: *mut *mut libpff_file_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_open(
        file: *mut libpff_file_t,
        filename: *const c_char,
        access_flags: c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_close(
        file: *mut libpff_file_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_is_corrupted(
        file: *mut libpff_file_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_size(
        file: *mut libpff_file_t,
        size: *mut u64,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_content_type(
        file: *mut libpff_file_t,
        content_type: *mut c_uchar,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_type(
        file: *mut libpff_file_t,
        file_type: *mut c_uchar,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_encryption_type(
        file: *mut libpff_file_t,
        encryption_type: *mut c_uchar,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_root_folder(
        file: *mut libpff_file_t,
        root_folder: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;

    pub(crate) fn libpff_item_free(
        item: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_item_get_identifier(
        item: *mut libpff_item_t,
        identifier: *mut u32,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_folder_get_number_of_sub_folders(
        folder: *mut libpff_item_t,
        number_of_sub_folders: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_folder_get_sub_folder(
        folder: *mut libpff_item_t,
        sub_folder_index: c_int,
        sub_folder: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_folder_get_number_of_sub_messages(
        folder: *mut libpff_item_t,
        number_of_sub_messages: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
}
