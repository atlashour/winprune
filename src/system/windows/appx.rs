use super::is_access_denied;
use crate::system::{AppxPackage, Outcome, Provisioned, SysError};
use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::{DeploymentResult, PackageManager, RemovalOptions};
use windows::core::HSTRING;

// Deployment error for packages Windows refuses to remove (ERROR_PACKAGE_NOT_REMOVABLE
// does not exist as a named constant in the SDK headers).
const HRESULT_NOT_REMOVABLE: i32 = 0x80073CFAu32 as i32;

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
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.full_name == b.full_name);
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
    finish(op.and_then(|op| op.join()))
}

fn finish(result: windows::core::Result<DeploymentResult>) -> Outcome {
    match result {
        Ok(r) => {
            let code = r.ExtendedErrorCode().map(|h| h.0).unwrap_or(0);
            if code == 0 {
                Outcome::Done
            } else if code == HRESULT_NOT_REMOVABLE {
                Outcome::Skipped("not removable".into())
            } else {
                let text = r.ErrorText().map(|t| t.to_string()).unwrap_or_default();
                Outcome::Failed(format!("0x{:08X} {}", code as u32, text.trim()))
            }
        }
        Err(e) if e.code().0 == HRESULT_NOT_REMOVABLE => Outcome::Skipped("not removable".into()),
        Err(e) => Outcome::Failed(e.message()),
    }
}
