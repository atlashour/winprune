use crate::catalog::Hive;
use crate::system::{Outcome, RegValue, SysError};
use std::io;
use winreg::enums::{
    HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, RegType,
};
use winreg::{RegKey, RegValue as RawValue};

fn root(hive: Hive) -> RegKey {
    RegKey::predef(match hive {
        Hive::Hklm => HKEY_LOCAL_MACHINE,
        Hive::Hkcu => HKEY_CURRENT_USER,
        Hive::Hkcr => HKEY_CLASSES_ROOT,
    })
}

pub fn read(hive: Hive, path: &str, name: &str) -> Result<Option<RegValue>, SysError> {
    let key = match root(hive).open_subkey_with_flags(path, KEY_READ) {
        Ok(key) => key,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            return Err(SysError::AccessDenied);
        }
        Err(e) => return Err(SysError::Other(e.to_string())),
    };
    match key.get_raw_value(name) {
        Ok(raw) => Ok(decode(&raw)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(SysError::AccessDenied),
        Err(e) => Err(SysError::Other(e.to_string())),
    }
}

fn decode(raw: &RawValue) -> Option<RegValue> {
    match raw.vtype {
        RegType::REG_DWORD if raw.bytes.len() >= 4 => Some(RegValue::Dword(u32::from_le_bytes([
            raw.bytes[0],
            raw.bytes[1],
            raw.bytes[2],
            raw.bytes[3],
        ]))),
        RegType::REG_SZ | RegType::REG_EXPAND_SZ => {
            let text: String = String::from_utf8_lossy(&raw.bytes).to_string();
            let wide: Vec<u16> = raw
                .bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let decoded = String::from_utf16_lossy(&wide);
            let decoded = decoded.trim_end_matches('\0').to_string();
            Some(RegValue::String(if decoded.is_empty() {
                text
            } else {
                decoded
            }))
        }
        _ => None,
    }
}

pub fn write(hive: Hive, path: &str, name: &str, value: &RegValue) -> Outcome {
    let key = match root(hive).create_subkey_with_flags(path, KEY_SET_VALUE) {
        Ok((key, _)) => key,
        Err(e) => return Outcome::Failed(format!("open key: {e}")),
    };
    let result = match value {
        RegValue::Dword(n) => key.set_value(name, n),
        RegValue::String(s) => key.set_value(name, s),
    };
    match result {
        Ok(()) => Outcome::Done,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

pub fn delete(hive: Hive, path: &str, name: &str) -> Outcome {
    match root(hive).open_subkey_with_flags(path, KEY_SET_VALUE) {
        Ok(key) => match key.delete_value(name) {
            Ok(()) => Outcome::Done,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Outcome::Skipped("value not present".into())
            }
            Err(e) => Outcome::Failed(e.to_string()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Outcome::Skipped("key not present".into()),
        Err(e) => Outcome::Failed(format!("open key: {e}")),
    }
}
