// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration between TPM devices and VMGS-backed storage.

#![forbid(unsafe_code)]

use mesh::MeshPayload;
use tpm_resources::ResolvedTpmDeviceConfig;
use tpm_resources::TpmDeviceConfigKind;
use tpm_resources::TpmVersion;
use vm_resource::AsyncResolveResource;
use vm_resource::IntoResource;
use vm_resource::PlatformResource;
use vm_resource::ResourceId;
use vm_resource::ResourceResolver;
use vm_resource::declare_static_async_resolver;
use vmgs_broker::VmgsBrokerError;
use vmgs_broker::VmgsClient;
use vmgs_broker::VmgsClientError;
use vmgs_broker::resolver::VmgsClientKind;
use vmgs_resources::VmgsFileHandle;

/// A handle that selects a TPM implementation version from VMGS state.
#[derive(MeshPayload)]
pub struct VmgsTpmDeviceConfigHandle {
    /// Requested TPM version, or `None` to select an existing version.
    pub requested_version: Option<TpmVersion>,
}

impl ResourceId<TpmDeviceConfigKind> for VmgsTpmDeviceConfigHandle {
    const ID: &'static str = "vmgs_tpm_device_config";
}

/// A resource resolver that selects a TPM version based on VMGS state.
pub struct VmgsTpmDeviceConfigResolver;

declare_static_async_resolver! {
    VmgsTpmDeviceConfigResolver,
    (TpmDeviceConfigKind, VmgsTpmDeviceConfigHandle),
}

/// Errors that can occur while resolving a VMGS-backed TPM configuration.
#[derive(Debug, thiserror::Error)]
pub enum VmgsTpmDeviceConfigResolverError {
    /// Error resolving the VMGS client.
    #[error("error resolving VMGS client")]
    Client(#[source] vm_resource::ResolveError),
    /// Error probing the VMGS for TPM state.
    #[error("error probing VMGS for TPM state")]
    Probe(#[source] VmgsClientError),
}

#[async_trait::async_trait]
impl AsyncResolveResource<TpmDeviceConfigKind, VmgsTpmDeviceConfigHandle>
    for VmgsTpmDeviceConfigResolver
{
    type Output = ResolvedTpmDeviceConfig;
    type Error = VmgsTpmDeviceConfigResolverError;

    async fn resolve(
        &self,
        resolver: &ResourceResolver,
        resource: VmgsTpmDeviceConfigHandle,
        _: &(),
    ) -> Result<Self::Output, Self::Error> {
        let vmgs = resolver
            .resolve::<VmgsClientKind, _>(PlatformResource.into_resource(), ())
            .await
            .map_err(VmgsTpmDeviceConfigResolverError::Client)?;

        let v185_allocated = is_file_allocated(&vmgs, tpm_nvram_file_id(TpmVersion::V185))
            .await
            .map_err(VmgsTpmDeviceConfigResolverError::Probe)?;
        let v138_allocated = is_file_allocated(&vmgs, tpm_nvram_file_id(TpmVersion::V138))
            .await
            .map_err(VmgsTpmDeviceConfigResolverError::Probe)?;

        let (version, conflicting_version) =
            select_tpm_version(resource.requested_version, v185_allocated, v138_allocated);
        if let Some(existing_version) = conflicting_version {
            tracing::warn!(
                ?version,
                ?existing_version,
                "requested TPM version has no state in VMGS, but another version does"
            );
        }

        Ok(ResolvedTpmDeviceConfig {
            version,
            nvram_store: VmgsFileHandle::new(tpm_nvram_file_id(version), true).into_resource(),
        })
    }
}

/// Returns the VMGS file ID used for the TPM version's NVRAM.
pub const fn tpm_nvram_file_id(version: TpmVersion) -> vmgs_format::FileId {
    match version {
        TpmVersion::V138 => vmgs_format::FileId::TPM_NVRAM,
        TpmVersion::V185 => vmgs_format::FileId::TPM_185_NVRAM,
    }
}

async fn is_file_allocated(
    vmgs: &VmgsClient,
    file_id: vmgs_format::FileId,
) -> Result<bool, VmgsClientError> {
    match vmgs.get_file_info(file_id).await {
        Ok(_) => Ok(true),
        Err(VmgsClientError::Vmgs(VmgsBrokerError::FileInfoNotAllocated)) => Ok(false),
        Err(err) => Err(err),
    }
}

fn select_tpm_version(
    requested_version: Option<TpmVersion>,
    v185_allocated: bool,
    v138_allocated: bool,
) -> (TpmVersion, Option<TpmVersion>) {
    if let Some(version) = requested_version {
        let (version_allocated, existing_version, existing_version_allocated) = match version {
            TpmVersion::V185 => (v185_allocated, TpmVersion::V138, v138_allocated),
            TpmVersion::V138 => (v138_allocated, TpmVersion::V185, v185_allocated),
        };
        let conflicting_version =
            (!version_allocated && existing_version_allocated).then_some(existing_version);
        (version, conflicting_version)
    } else if v185_allocated {
        (TpmVersion::V185, None)
    } else if v138_allocated {
        (TpmVersion::V138, None)
    } else {
        (TpmVersion::V185, None)
    }
}

#[cfg(test)]
mod tests {
    use super::select_tpm_version;
    use test_with_tracing::test;
    use tpm_resources::TpmVersion;

    #[test]
    fn tpm_version_selection() {
        assert_eq!(
            select_tpm_version(None, false, false),
            (TpmVersion::V185, None)
        );
        assert_eq!(
            select_tpm_version(None, false, true),
            (TpmVersion::V138, None)
        );
        assert_eq!(
            select_tpm_version(None, true, false),
            (TpmVersion::V185, None)
        );
        assert_eq!(
            select_tpm_version(None, true, true),
            (TpmVersion::V185, None)
        );

        assert_eq!(
            select_tpm_version(Some(TpmVersion::V185), false, true),
            (TpmVersion::V185, Some(TpmVersion::V138))
        );
        assert_eq!(
            select_tpm_version(Some(TpmVersion::V138), true, false),
            (TpmVersion::V138, Some(TpmVersion::V185))
        );
        assert_eq!(
            select_tpm_version(Some(TpmVersion::V185), true, true),
            (TpmVersion::V185, None)
        );
        assert_eq!(
            select_tpm_version(Some(TpmVersion::V138), false, false),
            (TpmVersion::V138, None)
        );
    }
}
