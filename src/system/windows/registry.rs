use crate::catalog::Hive;
use crate::system::{Outcome, RegValue, SysError};
use std::io;
use winreg::enums::{
    HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, RegType,
};
use winreg::types::FromRegValue;
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

pub fn write(hive: Hive, path: &str, name: &str, value: &RegValue) -> Outcome {
    let key = match root(hive).create_subkey_with_flags(path, KEY_SET_VALUE) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(vtype: RegType, bytes: &[u8]) -> RawValue {
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
}
