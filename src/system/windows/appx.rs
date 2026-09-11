use super::is_access_denied;
use super::registry::ensure_key;
use crate::system::{AppxPackage, Outcome, Provisioned, SysError};
use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::{DeploymentResult, PackageManager, RemovalOptions};
use windows::Win32::Foundation::{
    E_NOINTERFACE, ERROR_INSTALL_PACKAGE_NOT_FOUND, ERROR_NOT_SUPPORTED, ERROR_REMOVE_FAILED,
};
use windows::core::{HRESULT, HSTRING};

// ERROR_REMOVE_FAILED is the generic code; the extended one carries the reason, and
// ERROR_NOT_SUPPORTED there means "part of Windows".
const REMOVE_FAILED: HRESULT = HRESULT::from_win32(ERROR_REMOVE_FAILED.0);
const NOT_SUPPORTED: HRESULT = HRESULT::from_win32(ERROR_NOT_SUPPORTED.0);
const NOT_INSTALLED: HRESULT = HRESULT::from_win32(ERROR_INSTALL_PACKAGE_NOT_FOUND.0);

// Marker key that keeps feature updates and new profiles from bringing a
// deprovisioned app back. Not documented as written by the WinRT call, so write it.
const DEPROVISIONED: &str =
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Appx\\AppxAllUserStore\\Deprovisioned";

fn manager() -> Result<PackageManager, SysError> {
    PackageManager::new().map_err(|e| SysError::Other(e.message()))
}

/// Packages for every user when elevated, otherwise the current user's.
pub fn installed() -> Result<Vec<AppxPackage>, SysError> {
    let pm = manager()?;
    let iter = match pm.FindPackages() {
        Ok(iter) => iter,
        Err(e) if is_access_denied(&e) => pm
            .FindPackagesByUserSecurityId(&HSTRING::new())
            .map_err(|e| SysError::Other(e.message()))?,
        Err(e) => return Err(SysError::Other(e.message())),
    };
    let mut out = Vec::new();
    for pkg in iter {
        if let Some(p) = describe(&pkg) {
            out.push(p);
        }
    }
    // The all-users list repeats a package once per user that has it registered.
    out.sort_by(|a, b| a.full_name.cmp(&b.full_name));
    out.dedup_by(|a, b| a.full_name == b.full_name);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn describe(pkg: &Package) -> Option<AppxPackage> {
    let id = pkg.Id().ok()?;
    let framework = pkg.IsFramework().unwrap_or(false);
    let system = pkg
        .SignatureKind()
        .map(|k| k == PackageSignatureKind::System)
        .unwrap_or(false);
    Some(AppxPackage {
        name: id.Name().ok()?.to_string(),
        full_name: id.FullName().ok()?.to_string(),
        family: id.FamilyName().ok()?.to_string(),
        non_removable: framework || system,
    })
}

pub fn provisioned() -> Result<Provisioned, SysError> {
    let pm = manager()?;
    match pm.FindProvisionedPackages() {
        Ok(list) => {
            let mut families = Vec::new();
            for pkg in list {
                if let Ok(id) = pkg.Id()
                    && let Ok(family) = id.FamilyName()
                {
                    families.push(family.to_string());
                }
            }
            families.sort();
            families.dedup();
            Ok(Provisioned::Known(families))
        }
        Err(e) if is_access_denied(&e) => Ok(Provisioned::NeedsElevation),
        Err(e) if e.code() == E_NOINTERFACE => Err(SysError::Other(
            "listing provisioned packages needs Windows 10 2004 or later".into(),
        )),
        Err(e) => Err(SysError::Other(e.message())),
    }
}

pub fn remove(full_name: &str) -> Outcome {
    let pm = match manager() {
        Ok(pm) => pm,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let op = pm.RemovePackageWithOptionsAsync(
        &HSTRING::from(full_name),
        RemovalOptions::RemoveForAllUsers,
    );
    finish(op.and_then(|op| op.join()))
}

pub fn deprovision(family: &str) -> Outcome {
    let pm = match manager() {
        Ok(pm) => pm,
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let op = pm.DeprovisionPackageForAllUsersAsync(&HSTRING::from(family));
    match finish(op.and_then(|op| op.join())) {
        Outcome::Done => match ensure_key(&format!("{DEPROVISIONED}\\{family}")) {
            Ok(()) => Outcome::Done,
            Err(e) => Outcome::Failed(format!("deprovisioned, but the marker key failed: {e}")),
        },
        other => other,
    }
}

fn finish(result: windows::core::Result<DeploymentResult>) -> Outcome {
    match result {
        Ok(r) => {
            let code = r.ExtendedErrorCode().unwrap_or(HRESULT(0));
            if code.0 == 0 {
                Outcome::Done
            } else if code == NOT_SUPPORTED {
                Outcome::Skipped("part of Windows".into())
            } else if code == NOT_INSTALLED {
                Outcome::Skipped("not installed".into())
            } else {
                let text = r.ErrorText().map(|t| t.to_string()).unwrap_or_default();
                Outcome::Failed(format!("0x{:08X} {}", code.0 as u32, text.trim()))
            }
        }
        Err(e) if e.code() == NOT_SUPPORTED || e.code() == REMOVE_FAILED => {
            Outcome::Skipped("part of Windows".into())
        }
        Err(e) if e.code() == NOT_INSTALLED => Outcome::Skipped("not installed".into()),
        Err(e) => Outcome::Failed(e.message()),
    }
}
