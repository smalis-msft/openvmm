// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration between TPM devices and VMGS-backed storage.

#![forbid(unsafe_code)]

use tpm_resources::TpmVersion;
use vmgs_format::FileId;

/// How the TPM version should be selected.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum TpmVersionSelection {
    /// Always use the specified version.
    Specified(TpmVersion),
    /// Prefer an existing VMGS file, falling back to the hinted version.
    Hint(TpmVersion),
}

/// The result of selecting a TPM version.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TpmVersionDecision {
    /// The version to use.
    pub version: TpmVersion,
    /// A different version whose VMGS file conflicts with an explicit choice.
    pub conflicting_version: Option<TpmVersion>,
}

/// How the real VMGS will be initialized after read-only inspection.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VmgsOpenMode {
    /// Treat an empty VMGS as having no state, but reject invalid contents.
    OnEmpty,
    /// Treat an empty or invalid VMGS as having no state.
    OnFailure,
    /// Treat state as absent unless the VMGS is already marked reprovisioned.
    Reprovision,
}

/// Returns the VMGS file ID used for the TPM version's NVRAM.
pub const fn tpm_nvram_file_id(version: TpmVersion) -> FileId {
    match version {
        TpmVersion::V138 => FileId::TPM_NVRAM,
        TpmVersion::V185 => FileId::TPM_185_NVRAM,
    }
}

/// Selects a TPM version based on the requested behavior and VMGS contents.
pub fn select_tpm_version(
    selection: TpmVersionSelection,
    mut is_file_allocated: impl FnMut(FileId) -> bool,
) -> TpmVersionDecision {
    match selection {
        TpmVersionSelection::Specified(version) => {
            let conflicting_version = if !is_file_allocated(tpm_nvram_file_id(version)) {
                let other_version = match version {
                    TpmVersion::V138 => TpmVersion::V185,
                    TpmVersion::V185 => TpmVersion::V138,
                };
                is_file_allocated(tpm_nvram_file_id(other_version)).then_some(other_version)
            } else {
                None
            };

            TpmVersionDecision {
                version,
                conflicting_version,
            }
        }
        TpmVersionSelection::Hint(fallback) => {
            let version = [TpmVersion::V185, TpmVersion::V138]
                .into_iter()
                .find(|&version| is_file_allocated(tpm_nvram_file_id(version)))
                .unwrap_or(fallback);
            TpmVersionDecision {
                version,
                conflicting_version: None,
            }
        }
    }
}

/// Resolves the TPM version from an optional VMGS probe disk.
pub async fn resolve_tpm_version(
    vmgs: Option<(disk_backend::Disk, VmgsOpenMode)>,
    specified_version: Option<TpmVersion>,
) -> Result<TpmVersion, vmgs::Error> {
    let selection = specified_version
        .map(TpmVersionSelection::Specified)
        .unwrap_or(TpmVersionSelection::Hint(TpmVersion::V185));
    let decision = if let Some((disk, open_mode)) = vmgs {
        let vmgs = match vmgs::Vmgs::open_read_only(disk, None).await {
            Ok(vmgs) if open_mode != VmgsOpenMode::Reprovision || vmgs.is_reprovisioned() => {
                Some(vmgs)
            }
            Ok(_) | Err(vmgs::Error::EmptyFile) => None,
            Err(_) if open_mode == VmgsOpenMode::OnFailure => None,
            Err(err) => return Err(err),
        };
        select_tpm_version(selection, |file_id| {
            vmgs.as_ref()
                .is_some_and(|vmgs| vmgs.check_file_allocated(file_id))
        })
    } else {
        select_tpm_version(selection, |_| false)
    };

    if let Some(existing_version) = decision.conflicting_version {
        tracing::warn!(
            requested_version = ?decision.version,
            ?existing_version,
            "VMGS contains TPM NVRAM for a different version"
        );
    }

    Ok(decision.version)
}

#[cfg(test)]
mod tests {
    use super::TpmVersionDecision;
    use super::TpmVersionSelection;
    use super::select_tpm_version;
    use super::tpm_nvram_file_id;
    use test_with_tracing::test;
    use tpm_resources::TpmVersion;

    #[test]
    fn hint_prefers_existing_state_then_fallback() {
        for (v185, v138, expected) in [
            (false, false, TpmVersion::V138),
            (false, true, TpmVersion::V138),
            (true, false, TpmVersion::V185),
            (true, true, TpmVersion::V185),
        ] {
            let decision =
                select_tpm_version(TpmVersionSelection::Hint(TpmVersion::V138), |file_id| {
                    (file_id == tpm_nvram_file_id(TpmVersion::V185) && v185)
                        || (file_id == tpm_nvram_file_id(TpmVersion::V138) && v138)
                });
            assert_eq!(
                decision,
                TpmVersionDecision {
                    version: expected,
                    conflicting_version: None,
                }
            );
        }

        assert_eq!(
            select_tpm_version(TpmVersionSelection::Hint(TpmVersion::V185), |_| false).version,
            TpmVersion::V185
        );
    }

    #[test]
    fn specified_version_only_reports_conflicting_state() {
        for (specified, existing) in [
            (TpmVersion::V185, TpmVersion::V138),
            (TpmVersion::V138, TpmVersion::V185),
        ] {
            assert_eq!(
                select_tpm_version(TpmVersionSelection::Specified(specified), |file_id| {
                    file_id == tpm_nvram_file_id(existing)
                }),
                TpmVersionDecision {
                    version: specified,
                    conflicting_version: Some(existing),
                }
            );
        }
    }
}
