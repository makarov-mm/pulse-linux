// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Arc;

/// Resolves user IDs to names through NSS, so accounts from LDAP or SSSD are found as well.
/// Each ID is looked up once.
#[derive(Default)]
pub struct Users {
    names: HashMap<u32, Arc<str>>,
}

impl Users {
    /// The account name, or the number itself for an ID without an account.
    pub fn name(&mut self, uid: u32) -> Arc<str> {
        self.names
            .entry(uid)
            .or_insert_with(|| lookup(uid).unwrap_or_else(|| uid.to_string()).into())
            .clone()
    }
}

fn lookup(uid: u32) -> Option<String> {
    let mut buffer = vec![0; 1024];
    loop {
        // SAFETY: `passwd` is plain data for which all-zero bytes are a valid value.
        let mut record: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found = std::ptr::null_mut();
        // SAFETY: every pointer refers to a live local, and the length passed is that of `buffer`.
        let status = unsafe { libc::getpwuid_r(uid, &mut record, buffer.as_mut_ptr(), buffer.len(), &mut found) };
        if status == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 4, 0);
            continue;
        }
        if status != 0 || found.is_null() {
            return None;
        }
        // SAFETY: on success `pw_name` points to a NUL-terminated string inside `buffer`, which is still alive.
        let name = unsafe { CStr::from_ptr(record.pw_name) };
        return Some(name.to_string_lossy().into_owned());
    }
}
