use super::WindowsSystem;
use super::profiles::DEFAULT_MOUNT;
use crate::system::{Outcome, RegRoot, RegValue, SysError};
use std::io;
use winreg::enums::{
    HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_SET_VALUE,
    RegType,
};
use winreg::types::FromRegValue;
use winreg::{RegKey, RegValue as RawValue};

/// Per-user class registrations live in a hive of their own (`HKU\<sid>_Classes`),
/// which is where `HKCU\Software\Classes` really points.
const CLASSES_PREFIX: &str = "software\\classes\\";

/// Turns a root plus catalog path into the key to open and the path inside it.
fn resolve(sys: &WindowsSystem, root: &RegRoot, path: &str) -> Result<(RegKey, String), SysError> {
    match root {
        RegRoot::Machine => Ok((RegKey::predef(HKEY_LOCAL_MACHINE), path.to_string())),
        RegRoot::Classes => Ok((RegKey::predef(HKEY_CLASSES_ROOT), path.to_string())),
        RegRoot::CurrentUser => Ok((RegKey::predef(HKEY_CURRENT_USER), path.to_string())),
        RegRoot::User { sid, .. } => user_root(sid, path),
        RegRoot::DefaultProfile => {
            if path.to_ascii_lowercase().starts_with(CLASSES_PREFIX) {
                return Err(SysError::NotLoaded);
            }
            sys.default_mount().map_err(|_| SysError::NotLoaded)?;
            user_root(DEFAULT_MOUNT, path)
        }
    }
}

fn user_root(mount: &str, path: &str) -> Result<(RegKey, String), SysError> {
    let users = RegKey::predef(HKEY_USERS);
    if let Some(rest) = strip_prefix_ci(path, CLASSES_PREFIX) {
        let classes = users
            .open_subkey_with_flags(format!("{mount}_Classes"), KEY_READ)
            .map_err(|_| SysError::NotLoaded)?;
        return Ok((classes, rest.to_string()));
    }
    let hive = users
        .open_subkey_with_flags(mount, KEY_READ)
        .map_err(|_| SysError::NotLoaded)?;
    Ok((hive, path.to_string()))
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

pub fn read(
    sys: &WindowsSystem,
    root: &RegRoot,
    path: &str,
    name: &str,
) -> Result<Option<RegValue>, SysError> {
    let (base, path) = resolve(sys, root, path)?;
    let key = match base.open_subkey_with_flags(&path, KEY_READ) {
        Ok(key) => key,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            return Err(SysError::AccessDenied);
        }
        Err(e) => return Err(SysError::Other(e.to_string())),
    };
    match key.get_raw_value(name) {
        Ok(raw) => Ok(Some(decode(&raw))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(SysError::AccessDenied),
        Err(e) => Err(SysError::Other(e.to_string())),
    }
}

/// A value that exists always decodes to something, so the planner can tell "absent"
/// from "present with a type we do not write".
fn decode(raw: &RawValue) -> RegValue {
    let other = || RegValue::Other(format!("{:?}", raw.vtype));
    match raw.vtype {
        RegType::REG_DWORD | RegType::REG_DWORD_BIG_ENDIAN => u32::from_reg_value(raw)
            .map(RegValue::Dword)
            .unwrap_or_else(|_| other()),
        RegType::REG_SZ | RegType::REG_EXPAND_SZ => String::from_reg_value(raw)
            .map(RegValue::String)
            .unwrap_or_else(|_| other()),
        _ => other(),
    }
}

pub fn write(
    sys: &WindowsSystem,
    root: &RegRoot,
    path: &str,
    name: &str,
    value: &RegValue,
) -> Outcome {
    let (base, path) = match resolve(sys, root, path) {
        Ok(r) => r,
        Err(SysError::NotLoaded) => return Outcome::Skipped("profile hive not loaded".into()),
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let key = match base.create_subkey_with_flags(&path, KEY_SET_VALUE) {
        Ok((key, _)) => key,
        Err(e) => return Outcome::Failed(format!("open key: {e}")),
    };
    let result = match value {
        RegValue::Dword(n) => key.set_value(name, n),
        RegValue::String(s) => key.set_value(name, s),
        RegValue::Other(kind) => return Outcome::Failed(format!("cannot write a {kind} value")),
    };
    match result {
        Ok(()) => Outcome::Done,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

pub fn delete(sys: &WindowsSystem, root: &RegRoot, path: &str, name: &str) -> Outcome {
    let (base, path) = match resolve(sys, root, path) {
        Ok(r) => r,
        Err(SysError::NotLoaded) => return Outcome::Skipped("profile hive not loaded".into()),
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    match base.open_subkey_with_flags(&path, KEY_SET_VALUE) {
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

/// Creates a key with no values. Used for the Deprovisioned markers.
pub fn ensure_key(path: &str) -> Result<(), String> {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .create_subkey_with_flags(path, KEY_SET_VALUE)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(vtype: RegType, bytes: &[u8]) -> RawValue<'_> {
        RawValue {
            bytes: bytes.to_vec().into(),
            vtype,
        }
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn decodes_dword_little_endian() {
        assert_eq!(
            decode(&raw(RegType::REG_DWORD, &[1, 0, 0, 0])),
            RegValue::Dword(1)
        );
    }

    #[test]
    fn decodes_strings_and_drops_trailing_nul() {
        assert_eq!(
            decode(&raw(RegType::REG_SZ, &utf16("abc\0"))),
            RegValue::String("abc".into())
        );
        assert_eq!(
            decode(&raw(RegType::REG_EXPAND_SZ, &utf16("%SystemRoot%\\x\0"))),
            RegValue::String("%SystemRoot%\\x".into())
        );
        assert_eq!(
            decode(&raw(RegType::REG_SZ, &utf16("\0"))),
            RegValue::String(String::new())
        );
    }

    #[test]
    fn other_types_are_present_but_opaque() {
        assert_eq!(
            decode(&raw(RegType::REG_BINARY, &[1, 2, 3])),
            RegValue::Other("REG_BINARY".into())
        );
        assert_eq!(
            decode(&raw(RegType::REG_QWORD, &[0; 8])),
            RegValue::Other("REG_QWORD".into())
        );
        assert_eq!(
            decode(&raw(RegType::REG_DWORD, &[1, 0])),
            RegValue::Other("REG_DWORD".into())
        );
    }

    #[test]
    fn classes_prefix_is_split_case_insensitively() {
        assert_eq!(
            strip_prefix_ci("Software\\Classes\\CLSID\\x", CLASSES_PREFIX),
            Some("CLSID\\x")
        );
        assert_eq!(strip_prefix_ci("Software\\Microsoft", CLASSES_PREFIX), None);
    }
}
