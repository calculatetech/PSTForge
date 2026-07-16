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

#[repr(C)]
pub(crate) struct libpff_record_set_t {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct libpff_record_entry_t {
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
    pub(crate) fn libpff_file_recover_items(
        file: *mut libpff_file_t,
        recovery_flags: c_uchar,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_number_of_orphan_items(
        file: *mut libpff_file_t,
        number_of_orphan_items: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_orphan_item_by_index(
        file: *mut libpff_file_t,
        orphan_item_index: c_int,
        orphan_item: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_number_of_recovered_items(
        file: *mut libpff_file_t,
        number_of_recovered_items: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_file_get_recovered_item_by_index(
        file: *mut libpff_file_t,
        recovered_item_index: c_int,
        recovered_item: *mut *mut libpff_item_t,
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
    pub(crate) fn libpff_item_get_type(
        item: *mut libpff_item_t,
        item_type: *mut c_uchar,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_item_get_number_of_record_sets(
        item: *mut libpff_item_t,
        number_of_record_sets: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_item_get_record_set_by_index(
        item: *mut libpff_item_t,
        record_set_index: c_int,
        record_set: *mut *mut libpff_record_set_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_item_get_entry_value_32bit(
        item: *mut libpff_item_t,
        record_set_index: c_int,
        entry_type: u32,
        entry_value: *mut u32,
        flags: c_uchar,
        error: *mut *mut libpff_error_t,
    ) -> c_int;

    pub(crate) fn libpff_record_set_free(
        record_set: *mut *mut libpff_record_set_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_set_get_number_of_entries(
        record_set: *mut libpff_record_set_t,
        number_of_entries: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_set_get_entry_by_index(
        record_set: *mut libpff_record_set_t,
        record_entry_index: c_int,
        record_entry: *mut *mut libpff_record_entry_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_set_get_entry_by_type(
        record_set: *mut libpff_record_set_t,
        entry_type: u32,
        value_type: u32,
        record_entry: *mut *mut libpff_record_entry_t,
        flags: c_uchar,
        error: *mut *mut libpff_error_t,
    ) -> c_int;

    pub(crate) fn libpff_record_entry_free(
        record_entry: *mut *mut libpff_record_entry_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_entry_get_entry_type(
        record_entry: *mut libpff_record_entry_t,
        entry_type: *mut u32,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_entry_get_value_type(
        record_entry: *mut libpff_record_entry_t,
        value_type: *mut u32,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_entry_get_data_size(
        record_entry: *mut libpff_record_entry_t,
        data_size: *mut usize,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_entry_read_buffer(
        record_entry: *mut libpff_record_entry_t,
        buffer: *mut u8,
        buffer_size: usize,
        error: *mut *mut libpff_error_t,
    ) -> isize;
    pub(crate) fn libpff_record_entry_get_data_as_32bit_integer(
        record_entry: *mut libpff_record_entry_t,
        value: *mut u32,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_entry_get_data_as_utf8_string_size(
        record_entry: *mut libpff_record_entry_t,
        string_size: *mut usize,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_record_entry_get_data_as_utf8_string(
        record_entry: *mut libpff_record_entry_t,
        string: *mut u8,
        string_size: usize,
        error: *mut *mut libpff_error_t,
    ) -> c_int;

    pub(crate) fn libpff_folder_get_utf8_name_size(
        folder: *mut libpff_item_t,
        string_size: *mut usize,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_folder_get_utf8_name(
        folder: *mut libpff_item_t,
        string: *mut u8,
        string_size: usize,
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
    pub(crate) fn libpff_folder_get_sub_message(
        folder: *mut libpff_item_t,
        sub_message_index: c_int,
        sub_message: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;

    pub(crate) fn libpff_message_get_entry_value_utf8_string_size(
        message: *mut libpff_item_t,
        entry_type: u32,
        string_size: *mut usize,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_message_get_entry_value_utf8_string(
        message: *mut libpff_item_t,
        entry_type: u32,
        string: *mut u8,
        string_size: usize,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_message_get_client_submit_time(
        message: *mut libpff_item_t,
        filetime: *mut u64,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_message_get_delivery_time(
        message: *mut libpff_item_t,
        filetime: *mut u64,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_message_get_number_of_attachments(
        message: *mut libpff_item_t,
        number_of_attachments: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_message_get_attachment(
        message: *mut libpff_item_t,
        attachment_index: c_int,
        attachment: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_message_get_recipients(
        message: *mut libpff_item_t,
        recipients: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;

    pub(crate) fn libpff_attachment_get_type(
        attachment: *mut libpff_item_t,
        attachment_type: *mut c_int,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_attachment_get_data_size(
        attachment: *mut libpff_item_t,
        size: *mut u64,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
    pub(crate) fn libpff_attachment_data_read_buffer(
        attachment: *mut libpff_item_t,
        buffer: *mut u8,
        buffer_size: usize,
        error: *mut *mut libpff_error_t,
    ) -> isize;
    pub(crate) fn libpff_attachment_data_seek_offset(
        attachment: *mut libpff_item_t,
        offset: i64,
        whence: c_int,
        error: *mut *mut libpff_error_t,
    ) -> i64;
    pub(crate) fn libpff_attachment_get_item(
        attachment: *mut libpff_item_t,
        attached_item: *mut *mut libpff_item_t,
        error: *mut *mut libpff_error_t,
    ) -> c_int;
}
